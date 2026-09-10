//! `account/rateLimits/read` 응답 모델.
//!
//! 필드 정의는 `codex app-server generate-json-schema` 가 내보내는
//! `GetAccountRateLimitsResponse` 를 따른다. 서버가 정규화해 둔
//! `rateLimitsByLimitId` 를 우선 쓰고, 없을 때만 하위호환용 `rateLimits`
//! 단일 뷰로 내려간다.

use chrono::{DateTime, Local, TimeZone};
use serde::Deserialize;

use crate::domain::usage::{ResetCredits, UsageLimit};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitsResponse {
    pub rate_limits: Snapshot,
    #[serde(default)]
    pub rate_limit_reset_credits: Option<ResetCreditsResponse>,
    #[serde(default)]
    pub rate_limits_by_limit_id: Option<std::collections::HashMap<String, Snapshot>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(default)]
    pub limit_id: Option<String>,
    /// 화면에 쓰는 이름. 기본 한도는 비어 있다.
    #[serde(default)]
    pub limit_name: Option<String>,
    #[serde(default)]
    pub primary: Option<Window>,
    #[serde(default)]
    pub secondary: Option<Window>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub used_percent: f64,
    #[serde(default)]
    pub window_duration_mins: Option<i64>,
    /// epoch seconds
    #[serde(default)]
    pub resets_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCreditsResponse {
    available_count: u64,
    #[serde(default)]
    credits: Option<Vec<ResetCredit>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetCredit {
    status: String,
    #[serde(default)]
    expires_at: Option<i64>,
}

impl ResetCreditsResponse {
    pub fn to_domain(&self) -> ResetCredits {
        ResetCredits {
            available_count: self.available_count,
            earliest_known_expires_at: self
                .credits
                .iter()
                .flatten()
                .filter(|credit| self.available_count > 0 && credit.status == "available")
                .filter_map(|credit| Local.timestamp_opt(credit.expires_at?, 0).single())
                .min(),
        }
    }
}

impl Window {
    pub fn resets_at_local(&self) -> Option<DateTime<Local>> {
        let ts = self.resets_at?;
        Local.timestamp_opt(ts, 0).single()
    }
}

impl RateLimitsResponse {
    /// 표시할 스냅샷 목록. 중복을 피하려고 다중 뷰가 있으면 그쪽만 쓴다.
    ///
    /// 순서는 이름 없는 기본 한도가 먼저, 그다음 이름순 — HashMap 이라
    /// 정렬하지 않으면 실행할 때마다 줄 순서가 바뀐다.
    pub fn snapshots(&self) -> Vec<&Snapshot> {
        match &self.rate_limits_by_limit_id {
            Some(map) if !map.is_empty() => {
                let mut v: Vec<&Snapshot> = map.values().collect();
                // Option 의 기본 정렬이 None 을 앞에 두므로 이름 없는 기본 한도가 먼저 온다
                v.sort_by_key(|s| (s.limit_name.as_deref(), s.limit_id.as_deref()));
                v
            }
            _ => vec![&self.rate_limits],
        }
    }
}

/// Keep all reported windows; missing duration only disables the time gauge.
pub fn to_limits(resp: &RateLimitsResponse) -> Vec<UsageLimit> {
    let mut out = Vec::new();
    for snap in resp.snapshots() {
        for (slot, w) in [
            ("primary", snap.primary.as_ref()),
            ("secondary", snap.secondary.as_ref()),
        ] {
            let Some(w) = w else { continue };
            let duration = w
                .window_duration_mins
                .filter(|mins| *mins > 0)
                .and_then(chrono::TimeDelta::try_minutes);
            let duration_id = w
                .window_duration_mins
                .map(|mins| mins.to_string())
                .unwrap_or_else(|| "unknown".into());
            let base_id = snap.limit_id.as_deref().unwrap_or("codex");
            out.push(UsageLimit::new(
                format!("{base_id}:{slot}:{duration_id}"),
                snap.limit_name.clone(),
                w.used_percent,
                None,
                false,
                duration,
                w.resets_at_local(),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 실제 `account/rateLimits/read` 응답에서 발췌.
    const SAMPLE: &str = r#"{
      "rateLimits": {
        "limitId":"codex","limitName":null,
        "primary":{"usedPercent":50,"windowDurationMins":10080,"resetsAt":1787196678},
        "secondary":null,"credits":{"hasCredits":false,"unlimited":false,"balance":"0"},
        "individualLimit":null,"spendControlReached":false,"planType":"pro",
        "rateLimitReachedType":null
      },
      "rateLimitsByLimitId": {
        "codex_bengalfox": {"limitId":"codex_bengalfox","limitName":"GPT-5.3-Codex-Spark",
          "primary":{"usedPercent":0,"windowDurationMins":10080,"resetsAt":1787657494},
          "secondary":null,"planType":"pro"},
        "codex": {"limitId":"codex","limitName":null,
          "primary":{"usedPercent":50,"windowDurationMins":10080,"resetsAt":1787196678},
          "secondary":null,"planType":"pro"}
      },
      "rateLimitResetCredits":{"availableCount":0,"credits":[]}
    }"#;

    fn parse() -> RateLimitsResponse {
        serde_json::from_str(SAMPLE).unwrap()
    }

    #[test]
    fn parses_real_response() {
        let r = parse();
        assert_eq!(r.rate_limits.primary.as_ref().unwrap().used_percent, 50.0);
        assert_eq!(r.snapshots().len(), 2);
    }

    /// 다중 뷰가 있으면 단일 뷰를 함께 넣지 않는다 — 같은 한도가 두 번 나오면 안 된다.
    #[test]
    fn does_not_duplicate_default_limit() {
        let limits = to_limits(&parse());
        assert_eq!(limits.len(), 2, "{limits:#?}");
    }

    /// 이름 없는 기본 한도가 먼저, 그다음 이름순 — 실행마다 순서가 바뀌면 안 된다.
    #[test]
    fn order_is_stable() {
        for _ in 0..20 {
            let limits = to_limits(&parse());
            assert_eq!(limits[0].scope, None);
            assert_eq!(limits[1].scope.as_deref(), Some("GPT-5.3-Codex-Spark"));
        }
    }

    #[test]
    fn usage_is_kept_as_domain_data() {
        let limits = to_limits(&parse());
        assert_eq!(limits[0].used_percent, 50.0);
        assert_eq!(limits[1].used_percent, 0.0);
        assert_eq!(limits[0].window_duration, Some(chrono::TimeDelta::days(7)));
    }

    /// 다중 뷰가 없으면 단일 뷰로 내려간다.
    #[test]
    fn falls_back_to_single_view() {
        let body = r#"{"rateLimits":{"limitId":"codex","limitName":null,
            "primary":{"usedPercent":42,"windowDurationMins":10080,"resetsAt":null},
            "secondary":null}}"#;
        let r: RateLimitsResponse = serde_json::from_str(body).unwrap();
        let limits = to_limits(&r);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].scope, None);
        assert_eq!(limits[0].used_percent, 42.0);
        assert!(limits[0].resets_at.is_none());
    }

    /// Short windows are meaningful limits too.
    #[test]
    fn preserves_short_and_weekly_windows() {
        let body = r#"{"rateLimits":{"limitId":"codex",
            "primary":{"usedPercent":10,"windowDurationMins":300},
            "secondary":{"usedPercent":60,"windowDurationMins":10080}}}"#;
        let r: RateLimitsResponse = serde_json::from_str(body).unwrap();
        let limits = to_limits(&r);
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].used_percent, 10.0);
        assert_eq!(limits[1].used_percent, 60.0);
    }

    /// `resetsAt` 이 없어도 시간 게이지 자리는 채운다.
    #[test]
    fn window_without_reset_keeps_its_duration() {
        let body = r#"{"rateLimits":{"limitId":"codex",
            "primary":{"usedPercent":0,"windowDurationMins":10080}}}"#;
        let r: RateLimitsResponse = serde_json::from_str(body).unwrap();
        let limits = to_limits(&r);
        assert_eq!(limits[0].window_duration, Some(chrono::TimeDelta::days(7)));
        assert!(limits[0].resets_at.is_none());
    }

    /// Missing duration does not discard the usage percentage.
    #[test]
    fn preserves_windows_without_duration() {
        let body = r#"{"rateLimits":{"limitId":"codex",
            "primary":{"usedPercent":10}}}"#;
        let r: RateLimitsResponse = serde_json::from_str(body).unwrap();
        let limits = to_limits(&r);
        assert_eq!(limits.len(), 1);
        assert!(limits[0].window_duration.is_none());
    }

    /// 리셋 시각이 있으면 남은 시간 게이지가 붙는다.
    #[test]
    fn attaches_reset_to_domain_window() {
        let at = (chrono::Local::now() + chrono::TimeDelta::days(2)).timestamp();
        let body = format!(
            r#"{{"rateLimits":{{"limitId":"codex",
            "primary":{{"usedPercent":30,"windowDurationMins":10080,"resetsAt":{at}}}}}}}"#
        );
        let r: RateLimitsResponse = serde_json::from_str(&body).unwrap();
        let limits = to_limits(&r);
        assert_eq!(limits[0].resets_at.unwrap().timestamp(), at);
    }
    #[test]
    fn credits_distinguish_absent_zero_and_count_only() {
        for missing in ["{}", r#"{"rateLimitResetCredits":null}"#] {
            let mut value: serde_json::Value = serde_json::from_str(missing).unwrap();
            value["rateLimits"] = serde_json::json!({});
            let response: RateLimitsResponse = serde_json::from_value(value).unwrap();
            assert!(response.rate_limit_reset_credits.is_none());
        }
        for details in [serde_json::Value::Null, serde_json::json!([])] {
            for count in [0, 2] {
                let response: RateLimitsResponse = serde_json::from_value(serde_json::json!({
                    "rateLimits": {}, "rateLimitResetCredits": {"availableCount": count, "credits": details}
                })).unwrap();
                let credits = response.rate_limit_reset_credits.unwrap().to_domain();
                assert_eq!(credits.available_count, count);
                assert!(credits.earliest_known_expires_at.is_none());
            }
        }
    }

    #[test]
    fn expiry_uses_only_available_details_and_keeps_server_count() {
        let response: RateLimitsResponse = serde_json::from_value(serde_json::json!({
            "rateLimits": {}, "rateLimitResetCredits": {"availableCount": 5, "credits": [
                {"status":"redeemed", "expiresAt": 100},
                {"status":"future-status", "expiresAt": 200},
                {"status":"available", "expiresAt": 400},
                {"status":"available", "expiresAt": 300},
                {"status":"available", "expiresAt": null},
                {"status":"available", "expiresAt": i64::MAX}
            ]}
        }))
        .unwrap();
        let mut raw = response.rate_limit_reset_credits.unwrap();
        let credits = raw.to_domain();
        assert_eq!(credits.available_count, 5);
        assert_eq!(credits.earliest_known_expires_at.unwrap().timestamp(), 300);
        raw.available_count = 0;
        assert!(raw.to_domain().earliest_known_expires_at.is_none());
    }
}
