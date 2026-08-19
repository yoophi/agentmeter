//! `/api/oauth/usage` 응답의 정규화 모델.
//!
//! 응답 최상위에는 `five_hour`, `seven_day_opus`, `tangelo`, `nimbus_quill`
//! 처럼 계정 종류에 따라 켜졌다 꺼지는 필드가 많다. 그 필드들에 의존하면
//! 계정이나 배포가 바뀔 때 바로 깨지므로, 렌더링에 필요한 값은 서버가
//! 이미 정규화해 둔 `limits` 배열에서만 읽는다.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::domain::usage::{Severity, UsageLimit};

/// 세션 한도 창 길이. 응답이 창 길이를 알려주지 않아 상수로 둔다
/// (`five_hour` 라는 필드명과 `/usage` 화면이 근거다).
const SESSION_WINDOW: chrono::TimeDelta = chrono::TimeDelta::hours(5);
/// 주간 한도 창 길이 (`seven_day`).
const WEEK_WINDOW: chrono::TimeDelta = chrono::TimeDelta::days(7);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageResponse {
    #[serde(default)]
    pub limits: Vec<Limit>,
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
        let raw = self.resets_at.as_deref()?;
        DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|dt| dt.with_timezone(&Local))
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
}
