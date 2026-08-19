//! `account/rateLimits/read` 응답 모델.
//!
//! 필드 정의는 `codex app-server generate-json-schema` 가 내보내는
//! `GetAccountRateLimitsResponse` 를 따른다. 서버가 정규화해 둔
//! `rateLimitsByLimitId` 를 우선 쓰고, 없을 때만 하위호환용 `rateLimits`
//! 단일 뷰로 내려간다.

use chrono::{DateTime, Local, TimeZone};
use serde::Deserialize;

use crate::meter::{
    Bar, Meter, Window as MeterWindow, pending_bar, resets_text, time_bar, window_title,
};

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

/// 화면 표현으로 변환. ccmeter 와 맞추기 위해 **소진율**(`N% used`)로 보여준다.
/// (Codex 의 `/status` 는 남은 비율로 표시하지만, 두 도구의 표기를 통일한다.)
///
/// 주간 창만 보여준다. Codex 는 짧은 창을 쓰지 않거나 늘 0 이라 줄만 차지한다.
pub fn to_meters(resp: &RateLimitsResponse, tz: &str) -> Vec<Meter> {
    // 한 번만 읽어 모든 항목이 같은 기준 시각을 쓰게 한다
    let now = Local::now();
    let mut out = Vec::new();
    for snap in resp.snapshots() {
        let scope = snap.limit_name.as_deref();
        for w in [snap.primary.as_ref(), snap.secondary.as_ref()]
            .into_iter()
            .flatten()
        {
            let Some(mins) = w.window_duration_mins else {
                continue;
            };
            if mins <= WEEKLY_MIN_MINS {
                continue;
            }
            // 타임스탬프는 한 번만 변환해서 게이지와 각주가 함께 쓴다
            let at = w.resets_at_local();
            let window = at.map(|at| MeterWindow {
                resets_at: at,
                len: chrono::TimeDelta::minutes(mins),
            });
            out.push(Meter {
                title: window_title(Some(mins), scope),
                usage: Bar::used(w.used_percent, None),
                window,
                time: Some(
                    window
                        .and_then(|w| time_bar(w.resets_at, w.len, now))
                        .unwrap_or_else(pending_bar),
                ),
                footnote: at.map(|at| resets_text(at, tz)),
                emphasized: false,
            });
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
        let meters = to_meters(&parse(), "Asia/Seoul");
        assert_eq!(meters.len(), 2, "{meters:#?}");
    }

    /// 이름 없는 기본 한도가 먼저, 그다음 이름순 — 실행마다 순서가 바뀌면 안 된다.
    #[test]
    fn order_is_stable() {
        for _ in 0..20 {
            let meters = to_meters(&parse(), "Asia/Seoul");
            assert_eq!(meters[0].title, "Current week (all models)");
            assert_eq!(meters[1].title, "Current week (GPT-5.3-Codex-Spark)");
        }
    }

    /// ccmeter 와 같은 `N% used` 표기여야 한다 (`% left` 아님).
    #[test]
    fn labels_use_the_shared_used_wording() {
        let meters = to_meters(&parse(), "Asia/Seoul");
        assert_eq!(meters[0].usage.label, "50% used");
        assert_eq!(meters[1].usage.label, "0% used");
        assert!(meters.iter().all(|m| !m.usage.label.contains("left")));
    }

    #[test]
    fn fill_follows_used_percent() {
        let meters = to_meters(&parse(), "Asia/Seoul");
        assert!((meters[0].usage.fill - 0.5).abs() < 1e-9);
        assert!((meters[1].usage.fill - 0.0).abs() < 1e-9);
    }

    /// 다중 뷰가 없으면 단일 뷰로 내려간다.
    #[test]
    fn falls_back_to_single_view() {
        let body = r#"{"rateLimits":{"limitId":"codex","limitName":null,
            "primary":{"usedPercent":42,"windowDurationMins":10080,"resetsAt":null},
            "secondary":null}}"#;
        let r: RateLimitsResponse = serde_json::from_str(body).unwrap();
        let meters = to_meters(&r, "Asia/Seoul");
        assert_eq!(meters.len(), 1);
        assert_eq!(meters[0].title, "Current week (all models)");
        assert_eq!(meters[0].usage.label, "42% used");
        assert!(meters[0].footnote.is_none(), "resetsAt 이 없으면 각주도 없다");
        // 시간 게이지는 자리를 지킨다 (창 미시작 표시)
        assert_eq!(meters[0].time.as_ref().unwrap().label, "not started");
    }

    /// 주간 창만 보여준다. 짧은 창은 줄만 차지하므로 제외한다.
    #[test]
    fn shows_only_weekly_windows() {
        let body = r#"{"rateLimits":{"limitId":"codex",
            "primary":{"usedPercent":10,"windowDurationMins":300},
            "secondary":{"usedPercent":60,"windowDurationMins":10080}}}"#;
        let r: RateLimitsResponse = serde_json::from_str(body).unwrap();
        let meters = to_meters(&r, "Asia/Seoul");
        assert_eq!(meters.len(), 1, "5시간 창은 빠져야 함: {meters:#?}");
        assert_eq!(meters[0].title, "Current week (all models)");
        assert_eq!(meters[0].usage.label, "60% used");
    }

    /// `resetsAt` 이 없어도 시간 게이지 자리는 채운다.
    #[test]
    fn window_without_reset_still_has_a_time_row() {
        let body = r#"{"rateLimits":{"limitId":"codex",
            "primary":{"usedPercent":0,"windowDurationMins":10080}}}"#;
        let r: RateLimitsResponse = serde_json::from_str(body).unwrap();
        let meters = to_meters(&r, "Asia/Seoul");
        assert_eq!(meters[0].time.as_ref().unwrap().label, "not started");
        assert!(meters[0].footnote.is_none());
    }

    /// 창 길이를 모르면 표시하지 않는다 — 시간 게이지를 만들 수 없다.
    #[test]
    fn skips_windows_without_duration() {
        let body = r#"{"rateLimits":{"limitId":"codex",
            "primary":{"usedPercent":10}}}"#;
        let r: RateLimitsResponse = serde_json::from_str(body).unwrap();
        assert!(to_meters(&r, "Asia/Seoul").is_empty());
    }

    /// 리셋 시각이 있으면 남은 시간 게이지가 붙는다.
    #[test]
    fn attaches_time_bar_from_reset() {
        let at = (chrono::Local::now() + chrono::TimeDelta::days(2)).timestamp();
        let body = format!(
            r#"{{"rateLimits":{{"limitId":"codex",
            "primary":{{"usedPercent":30,"windowDurationMins":10080,"resetsAt":{at}}}}}}}"#
        );
        let r: RateLimitsResponse = serde_json::from_str(&body).unwrap();
        let meters = to_meters(&r, "Asia/Seoul");
        let time = meters[0].time.as_ref().expect("시간 게이지가 있어야 함");
        assert!(time.label.starts_with("1 day 23 hour") || time.label.starts_with("2 day 0 hour"),
            "{}", time.label);
        // 7일 창에서 2일 남았으면 5/7 만큼 지났다
        assert!((time.fill - 5.0 / 7.0).abs() < 0.01, "fill={}", time.fill);
    }
}
