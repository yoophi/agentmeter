//! `/api/oauth/usage` 응답의 정규화 모델.
//!
//! 응답 최상위에는 `five_hour`, `seven_day_opus`, `tangelo`, `nimbus_quill`
//! 처럼 계정 종류에 따라 켜졌다 꺼지는 필드가 많다. 그 필드들에 의존하면
//! 계정이나 배포가 바뀔 때 바로 깨지므로, 렌더링에 필요한 값은 서버가
//! 이미 정규화해 둔 `limits` 배열에서만 읽는다.
//!
//! 예외는 `cedar_ember`(세션 한도 리셋 권한) 하나다. `limits` 에는 이 정보가
//! 없고, `?cedar_ember=1` 로 요청했을 때만 채워진다.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::domain::usage::{ResetCredits, Severity, UsageLimit};

/// 세션 한도 창 길이. 응답이 창 길이를 알려주지 않아 상수로 둔다
/// (`five_hour` 라는 필드명과 `/usage` 화면이 근거다).
const SESSION_WINDOW: chrono::TimeDelta = chrono::TimeDelta::hours(5);
/// 주간 한도 창 길이 (`seven_day`).
const WEEK_WINDOW: chrono::TimeDelta = chrono::TimeDelta::days(7);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageResponse {
    #[serde(default)]
    pub limits: Vec<Limit>,
    /// 세션 한도 리셋 권한. Claude Code 자체 캐시에는 없으므로 선택값이다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cedar_ember: Option<CedarEmber>,
}

/// `cedar_ember` 블록 — Claude Code 의 "Reset your session limit now".
///
/// 필드 정의는 Claude Code 2.1.280 이 이 블록을 검증하는 스키마를 따른다.
/// 렌더링에 쓰지 않는 `exhausted`, `cooldown_until`, `event_props` 등은 받지 않는다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CedarEmber {
    #[serde(default)]
    pub eligible: bool,
    #[serde(default)]
    pub grants: Vec<ResetGrant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetGrant {
    #[serde(default)]
    pub resets_left: u64,
    #[serde(default)]
    pub ends_at: Option<String>,
    #[serde(default)]
    pub paused: bool,
}

impl CedarEmber {
    /// 자격이 없는 계정(`eligible: false`)은 표시하지 않는다 — 0 으로 보이면
    /// "다 썼다"는 뜻으로 읽히지만 실제로는 기능이 꺼져 있는 것이다.
    pub fn to_domain(&self) -> Option<ResetCredits> {
        if !self.eligible {
            return None;
        }
        let usable = self
            .grants
            .iter()
            .filter(|grant| !grant.paused && grant.resets_left > 0);
        Some(ResetCredits {
            available_count: usable.clone().map(|grant| grant.resets_left).sum(),
            earliest_known_expires_at: usable
                .filter_map(|grant| parse_local(grant.ends_at.as_deref()?))
                .min(),
        })
    }
}

impl UsageResponse {
    pub fn reset_credits(&self) -> Option<ResetCredits> {
        self.cedar_ember.as_ref()?.to_domain()
    }
}

fn parse_local(raw: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Local))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limit {
    /// `session` | `weekly_all` | `weekly_scoped` | (그 외 미래 값)
    pub kind: String,
    // 참고: 응답에는 `group`, `scope.surface` 등도 있으나 렌더링에 쓰지 않으므로
    // 받지 않는다. serde 는 모르는 필드를 무시하므로 파싱에는 영향이 없다.
    #[serde(default)]
    pub percent: f64,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub resets_at: Option<String>,
    #[serde(default)]
    pub scope: Option<Scope>,
    /// 지금 실제로 적용 중인 한도인지
    #[serde(default)]
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    #[serde(default)]
    pub model: Option<ScopeModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeModel {
    #[serde(default)]
    pub display_name: Option<String>,
}

impl Limit {
    pub fn model_name(&self) -> Option<&str> {
        self.scope
            .as_ref()?
            .model
            .as_ref()?
            .display_name
            .as_deref()
            .filter(|s| !s.is_empty())
    }

    pub fn severity(&self) -> Severity {
        match self.severity.as_deref() {
            Some("normal") => Severity::Normal,
            Some("warning" | "warn" | "elevated") => Severity::Warning,
            Some("critical" | "exceeded" | "blocked") => Severity::Critical,
            // 처음 보는 severity 는 소진율로 판단
            _ => Severity::from_used_percent(self.percent),
        }
    }

    pub fn resets_at_local(&self) -> Option<DateTime<Local>> {
        parse_local(self.resets_at.as_deref()?)
    }
}

/// 한도 종류로 창 길이를 정한다.
///
/// 이 응답은 창 길이를 알려주지 않는다. `five_hour` / `seven_day` 라는
/// 필드명과 `/usage` 화면이 근거다. 모르는 종류면 시간 게이지를 만들지 않는다.
fn window_for(kind: &str) -> Option<chrono::TimeDelta> {
    match kind {
        "session" => Some(SESSION_WINDOW),
        k if k.starts_with("weekly") => Some(WEEK_WINDOW),
        _ => None,
    }
}

/// 서버가 준 한도를 공급자·화면에 독립적인 도메인 값으로 옮긴다.
pub fn to_limits(limits: &[Limit]) -> Vec<UsageLimit> {
    let weekly_reset = limits
        .iter()
        .filter(|limit| limit.kind.starts_with("weekly"))
        .find_map(Limit::resets_at_local);
    limits
        .iter()
        .map(|l| {
            let scope = l.model_name().map(str::to_string);
            let id = format!("{}:{}", l.kind, scope.as_deref().unwrap_or("all"));
            UsageLimit::new(
                id,
                scope,
                l.percent,
                Some(l.severity()),
                l.is_active,
                window_for(&l.kind),
                l.resets_at_local().or_else(|| {
                    if l.kind.starts_with("weekly") {
                        weekly_reset
                    } else {
                        None
                    }
                }),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_weekly_limit_inherits_the_group_reset() {
        let response: UsageResponse = serde_json::from_str(
            r#"{"limits":[
                {"kind":"weekly_all","percent":1,
                 "resets_at":"2026-08-25T16:00:00Z"},
                {"kind":"weekly_scoped","percent":0,"resets_at":null,
                 "scope":{"model":{"display_name":"Fable"}}}
            ]}"#,
        )
        .unwrap();

        let limits = to_limits(&response.limits);
        assert_eq!(limits[0].window(), limits[1].window());
        assert_eq!(limits[1].scope.as_deref(), Some("Fable"));
    }

    fn reset_credits(cedar_ember: &str) -> Option<ResetCredits> {
        let body = format!(r#"{{"limits":[],"cedar_ember":{cedar_ember}}}"#);
        serde_json::from_str::<UsageResponse>(&body)
            .unwrap()
            .reset_credits()
    }

    /// 실제 응답 — 자격이 없으면 grants 가 비어 있다.
    #[test]
    fn ineligible_account_shows_no_reset_credits() {
        let credits = reset_credits(
            r#"{"eligible":false,"ineligible_reason":"surface","at_limit":false,
                "exhausted":[],"grants":[],"next_grant_id":null,
                "weekly_resets_at":null,"cooldown_until":null,"event_props":null}"#,
        );
        assert!(credits.is_none());
    }

    #[test]
    fn usable_grants_are_summed_with_the_earliest_expiry() {
        let credits = reset_credits(
            r#"{"eligible":true,"grants":[
                {"id":"weekly","resets_total":1,"resets_left":1,
                 "ends_at":"2026-09-30T16:00:00Z","clears":["five_hour"]},
                {"id":"promo","resets_total":2,"resets_left":2,
                 "ends_at":"2026-09-25T00:00:00Z"},
                {"id":"spent","resets_total":1,"resets_left":0,
                 "ends_at":"2026-09-24T00:00:00Z"},
                {"id":"held","resets_total":1,"resets_left":1,"paused":true,
                 "ends_at":"2026-09-23T00:00:00Z"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(credits.available_count, 3);
        assert_eq!(
            credits
                .earliest_known_expires_at
                .unwrap()
                .to_utc()
                .to_rfc3339(),
            "2026-09-25T00:00:00+00:00"
        );
    }

    #[test]
    fn eligible_account_without_grants_reports_zero() {
        let credits = reset_credits(r#"{"eligible":true,"grants":[]}"#).unwrap();
        assert_eq!(credits.available_count, 0);
        assert!(credits.earliest_known_expires_at.is_none());
    }

    #[test]
    fn missing_or_null_block_is_not_an_error() {
        let response: UsageResponse =
            serde_json::from_str(r#"{"limits":[],"cedar_ember":null}"#).unwrap();
        assert!(response.reset_credits().is_none());
    }
}
