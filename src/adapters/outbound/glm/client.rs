//! `monitor/usage/quota/limit` 호출.
//!
//! 공식 문서에 없는 내부 엔드포인트다(공식 플러그인이 쓰는 경로). 스키마가
//! 바뀌거나 막힐 수 있으므로 실패는 모두 오류 상태로 표현한다.

use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::auth;
use super::model::Envelope;
use crate::application::FetchError;

const ENDPOINT: &str = "https://api.z.ai/api/monitor/usage/quota/limit";
const USER_AGENT: &str = concat!("agentmeter/", env!("CARGO_PKG_VERSION"));
const TIMEOUT: Duration = Duration::from_secs(15);

/// 키를 매번 새로 읽는다. 다른 도구가 키를 갱신하면 다음 조회에서 따라간다.
pub(crate) fn fetch() -> Result<Envelope, FetchError> {
    let key = auth::api_key().map_err(FetchError::Other)?;
    fetch_with(&key)
}

/// Agent 를 한 번만 만든다. 새로 만들면 rustls 설정이 매번 다시 구성된다.
fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build()
            .new_agent()
    })
}

fn fetch_with(key: &str) -> Result<Envelope, FetchError> {
    let mut resp = agent()
        .get(ENDPOINT)
        // 이 API 는 Bearer 접두어 없이 키를 그대로 받는다. 공식 플러그인도 같다.
        .header("Authorization", key)
        .header("Accept-Language", "en-US,en")
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .call()
        .map_err(|error| {
            FetchError::Other(anyhow::Error::new(error).context("quota 엔드포인트 호출 실패"))
        })?;

    let status = resp.status().as_u16();
    if status != 200 {
        return Err(status_error(status));
    }

    let body = resp
        .body_mut()
        .read_to_string()
        .context("응답 본문을 읽지 못했습니다")
        .map_err(FetchError::Other)?;

    parse_body(&body).map_err(FetchError::Other)
}

fn status_error(status: u16) -> FetchError {
    match status {
        401 | 403 => FetchError::Unauthorized(auth::hint().to_string()),
        429 => FetchError::Other(anyhow::anyhow!(
            "조회가 제한되었습니다 (HTTP 429). 잠시 후 다시 시도하세요"
        )),
        other => FetchError::Other(anyhow::anyhow!("서버가 HTTP {other} 를 반환했습니다")),
    }
}

fn parse_body(body: &str) -> Result<Envelope> {
    let parsed: Envelope = serde_json::from_str(body).context("quota 응답 파싱 실패")?;
    // HTTP 200 이어도 본문 code 로 실패를 알리는 API 다.
    if parsed.code != 200 {
        let message = parsed
            .msg
            .unwrap_or_else(|| format!("code {}", parsed.code));
        bail!("{message}");
    }
    let Some(data) = &parsed.data else {
        bail!("응답에 data 가 없습니다 (스키마가 변경되었을 수 있습니다)");
    };
    if data.limits.is_empty() {
        bail!("응답에 limits 항목이 없습니다 (스키마가 변경되었을 수 있습니다)");
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 실제 `quota/limit` 응답에서 발췌.
    const REAL: &str = r#"{"code":200,"msg":"Operation successful","data":{"limits":[
        {"type":"TIME_LIMIT","unit":5,"number":1,"usage":4000,"currentValue":0,
         "remaining":4000,"percentage":0,"nextResetTime":1790642457999,
         "usageDetails":[{"modelCode":"search-prime","usage":0}]},
        {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":4,
         "nextResetTime":1789373822847}],"level":"max"},"success":true}"#;

    #[test]
    fn the_real_response_parses() {
        let parsed = parse_body(REAL).unwrap();
        let data = parsed.data.unwrap();
        assert_eq!(data.limits.len(), 2);
        assert_eq!(data.level.as_deref(), Some("max"));
    }

    #[test]
    fn a_failing_code_becomes_an_error_even_on_http_200() {
        let error = parse_body(r#"{"code":401,"msg":"unauthorized","success":false}"#).unwrap_err();
        assert!(error.to_string().contains("unauthorized"), "{error}");
    }

    #[test]
    fn an_empty_limits_list_is_treated_as_a_schema_change() {
        let error = parse_body(r#"{"code":200,"data":{"limits":[]},"success":true}"#).unwrap_err();
        assert!(error.to_string().contains("limits"), "{error}");
    }

    #[test]
    fn auth_failures_ask_for_a_key_instead_of_a_bare_status() {
        assert!(matches!(status_error(401), FetchError::Unauthorized(_)));
        assert!(matches!(status_error(403), FetchError::Unauthorized(_)));
        assert!(status_error(429).to_string().contains("429"));
    }
}
