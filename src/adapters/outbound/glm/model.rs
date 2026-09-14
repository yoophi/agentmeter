//! quota 응답을 도메인 한도로 옮긴다.

use chrono::{DateTime, Local, TimeDelta, TimeZone};
use serde::Deserialize;

use crate::domain::usage::UsageLimit;

/// 토큰 창. 플랜의 주 한도이므로 화면에서 강조한다.
const TOKENS: &str = "TOKENS_LIMIT";
/// MCP 도구 호출 한도.
const TOOLS: &str = "TIME_LIMIT";

#[derive(Debug, Deserialize)]
pub(crate) struct Envelope {
    pub code: i64,
    pub msg: Option<String>,
    pub data: Option<Data>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Data {
    #[serde(default)]
    pub limits: Vec<Limit>,
    /// 플랜 등급(lite/pro/max). 표시할 자리가 아직 없어 읽기만 한다.
    #[allow(dead_code)]
    pub level: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Limit {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// 창 길이의 단위 코드. `number` 와 곱해 창 길이가 된다.
    pub unit: Option<i64>,
    pub number: Option<i64>,
    pub percentage: Option<f64>,
    /// 소진 비율을 직접 주지 않을 때 쓰는 값들.
    pub usage: Option<f64>,
    #[serde(rename = "currentValue")]
    pub current_value: Option<f64>,
    /// ms epoch.
    #[serde(rename = "nextResetTime")]
    pub next_reset_time: Option<i64>,
}

/// 응답 순서와 무관하게 의미로 id 를 정한다.
///
/// 자리(slot)를 id 에 넣으면 공급자가 순서를 바꿀 때 히스토리가 끊긴다.
fn limit_id(kind: &str) -> Option<&'static str> {
    match kind {
        TOKENS => Some("glm:tokens"),
        TOOLS => Some("glm:tools"),
        _ => None,
    }
}

/// 확인된 단위만 창 길이로 옮긴다.
///
/// 실측으로 3=시간(5시간 토큰 창), 5=월(1개월 도구 창)을 확인했다. 모르는
/// 단위는 `None` 이라 시간 게이지만 빠지고 소진율과 리셋 시각은 그대로 나온다.
fn window_duration(unit: Option<i64>, number: Option<i64>) -> Option<TimeDelta> {
    let number = number.filter(|value| *value > 0)?;
    match unit? {
        3 => TimeDelta::try_hours(number),
        // 달은 길이가 일정하지 않다. 경과 게이지를 위한 근사값이고, 창을 가르는
        // 기준은 어차피 리셋 시각이라 며칠의 오차는 히스토리를 섞지 않는다.
        5 => TimeDelta::try_days(number * 30),
        _ => None,
    }
}

/// `percentage` 가 없으면 사용량/한도로 계산한다.
fn used_percent(limit: &Limit) -> f64 {
    if let Some(percentage) = limit.percentage {
        return percentage;
    }
    match (limit.current_value, limit.usage) {
        (Some(used), Some(total)) if total > 0.0 => used / total * 100.0,
        _ => 0.0,
    }
}

fn resets_at(ms: Option<i64>) -> Option<DateTime<Local>> {
    let ms = ms.filter(|value| *value > 0)?;
    Local.timestamp_millis_opt(ms).single()
}

pub(crate) fn to_limits(data: &Data) -> Vec<UsageLimit> {
    let mut out = Vec::new();
    for limit in &data.limits {
        let Some(kind) = limit.kind.as_deref() else {
            continue;
        };
        let Some(id) = limit_id(kind) else {
            continue;
        };
        let scope = match kind {
            // 토큰 창은 제목만으로 충분하다 — "Current session".
            TOKENS => None,
            _ => Some("MCP tools".to_string()),
        };
        out.push(UsageLimit::new(
            id,
            scope,
            used_percent(limit),
            None,
            kind == TOKENS,
            window_duration(limit.unit, limit.number),
            resets_at(limit.next_reset_time),
        ));
    }
    // 응답 순서는 공급자 마음이다. 자주 보는 짧은 창을 위에 둔다.
    out.sort_by_key(|limit| limit.window_duration.unwrap_or(TimeDelta::MAX));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Data {
        serde_json::from_str::<Envelope>(raw).unwrap().data.unwrap()
    }

    const REAL: &str = r#"{"code":200,"data":{"limits":[
        {"type":"TIME_LIMIT","unit":5,"number":1,"usage":4000,"currentValue":0,
         "remaining":4000,"percentage":0,"nextResetTime":1790642457999},
        {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":4,
         "nextResetTime":1789373822847}],"level":"max"}}"#;

    #[test]
    fn the_token_window_becomes_the_active_five_hour_limit() {
        let limits = to_limits(&parse(REAL));
        let tokens = limits
            .iter()
            .find(|limit| limit.id.as_str() == "glm:tokens")
            .expect("토큰 한도가 있어야 함");
        assert_eq!(tokens.used_percent, 4.0);
        assert_eq!(tokens.window_duration, Some(TimeDelta::hours(5)));
        assert!(tokens.active, "플랜의 주 한도는 강조되어야 함");
        assert!(tokens.scope.is_none(), "5시간 창은 제목만으로 충분하다");
    }

    #[test]
    fn the_tool_window_is_scoped_so_it_is_not_labelled_all_models() {
        let limits = to_limits(&parse(REAL));
        let tools = limits
            .iter()
            .find(|limit| limit.id.as_str() == "glm:tools")
            .expect("도구 한도가 있어야 함");
        assert_eq!(tools.scope.as_deref(), Some("MCP tools"));
        assert!(!tools.active);
        assert_eq!(tools.window_duration, Some(TimeDelta::days(30)));
    }

    #[test]
    fn the_short_window_is_listed_first_whatever_the_response_order() {
        let limits = to_limits(&parse(REAL));
        assert_eq!(
            limits[0].id.as_str(),
            "glm:tokens",
            "5시간 창이 먼저여야 함"
        );
        assert_eq!(limits[1].id.as_str(), "glm:tools");
    }

    #[test]
    fn ids_do_not_depend_on_response_order() {
        let reversed = r#"{"code":200,"data":{"limits":[
            {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":4},
            {"type":"TIME_LIMIT","unit":5,"number":1,"percentage":0}]}}"#;
        let ids: Vec<String> = to_limits(&parse(reversed))
            .iter()
            .map(|limit| limit.id.as_str().to_string())
            .collect();
        assert_eq!(
            ids,
            vec!["glm:tokens", "glm:tools"],
            "자리가 아니라 종류로 id 를 정한다"
        );
    }

    #[test]
    fn an_unknown_unit_keeps_the_percentage_and_drops_only_the_time_gauge() {
        let odd = r#"{"code":200,"data":{"limits":[
            {"type":"TOKENS_LIMIT","unit":99,"number":7,"percentage":12}]}}"#;
        let limits = to_limits(&parse(odd));
        assert_eq!(limits[0].used_percent, 12.0);
        assert_eq!(limits[0].window_duration, None);
    }

    #[test]
    fn an_unknown_limit_type_is_skipped_rather_than_guessed() {
        let odd = r#"{"code":200,"data":{"limits":[{"type":"FUTURE_LIMIT","percentage":50}]}}"#;
        assert!(to_limits(&parse(odd)).is_empty());
    }

    #[test]
    fn a_missing_percentage_falls_back_to_the_counters() {
        let counted = r#"{"code":200,"data":{"limits":[
            {"type":"TIME_LIMIT","unit":5,"number":1,"usage":4000,"currentValue":1000}]}}"#;
        assert_eq!(to_limits(&parse(counted))[0].used_percent, 25.0);
    }
}
