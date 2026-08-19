//! `account/rateLimits/read` 응답 모델.
//!
//! 필드 정의는 `codex app-server generate-json-schema` 가 내보내는
//! `GetAccountRateLimitsResponse` 를 따른다. 서버가 정규화해 둔
//! `rateLimitsByLimitId` 를 우선 쓰고, 없을 때만 하위호환용 `rateLimits`
//! 단일 뷰로 내려간다.

use chrono::{DateTime, Local, TimeZone};
use serde::Deserialize;

use crate::domain::usage::UsageLimit;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitsResponse {
    pub rate_limits: Snapshot,
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

/// 주간 창으로 볼 최소 길이 (하루 초과). 이보다 짧은 창은 표시하지 않는다.
const WEEKLY_MIN_MINS: i64 = 60 * 24;

/// 주간 창만 보여준다. Codex 는 짧은 창을 쓰지 않거나 늘 0 이라 줄만 차지한다.
pub fn to_limits(resp: &RateLimitsResponse) -> Vec<UsageLimit> {
    let mut out = Vec::new();
    for snap in resp.snapshots() {
        for (slot, w) in [
            ("primary", snap.primary.as_ref()),
            ("secondary", snap.secondary.as_ref()),
        ] {
            let Some(w) = w else { continue };
            let Some(mins) = w.window_duration_mins else {
                continue;
            };
            if mins <= WEEKLY_MIN_MINS {
                continue;
            }
            let base_id = snap.limit_id.as_deref().unwrap_or("codex");
            out.push(UsageLimit::new(
                format!("{base_id}:{slot}:{mins}"),
                snap.limit_name.clone(),
                w.used_percent,
                None,
                false,
                Some(chrono::TimeDelta::minutes(mins)),
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

    /// 주간 창만 보여준다. 짧은 창은 줄만 차지하므로 제외한다.
    #[test]
    fn shows_only_weekly_windows() {
        let body = r#"{"rateLimits":{"limitId":"codex",
            "primary":{"usedPercent":10,"windowDurationMins":300},
            "secondary":{"usedPercent":60,"windowDurationMins":10080}}}"#;
        let r: RateLimitsResponse = serde_json::from_str(body).unwrap();
        let limits = to_limits(&r);
        assert_eq!(limits.len(), 1, "5시간 창은 빠져야 함: {limits:#?}");
        assert_eq!(limits[0].used_percent, 60.0);
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

    /// 창 길이를 모르면 표시하지 않는다 — 시간 게이지를 만들 수 없다.
    #[test]
    fn skips_windows_without_duration() {
        let body = r#"{"rateLimits":{"limitId":"codex",
            "primary":{"usedPercent":10}}}"#;
        let r: RateLimitsResponse = serde_json::from_str(body).unwrap();
        assert!(to_limits(&r).is_empty());
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
}
