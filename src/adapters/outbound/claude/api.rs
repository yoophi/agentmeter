//! `/api/oauth/usage` 호출.

use anyhow::{Context, Result, bail};
use std::time::Duration;

use super::auth::{self, Credentials};
use super::model::UsageResponse;
use crate::application::FetchError;

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const USER_AGENT: &str = concat!("ccmeter/", env!("CARGO_PKG_VERSION"));
const TIMEOUT: Duration = Duration::from_secs(15);

/// 자격증명을 매번 새로 읽어서 호출한다.
/// Claude Code 가 토큰을 갱신하면 다음 폴링에서 자동으로 따라간다.
pub fn fetch_response() -> Result<UsageResponse, FetchError> {
    let creds = auth::load().map_err(FetchError::Other)?;
    if creds.is_expired() {
        return Err(FetchError::Unauthorized(auth::reauth_hint().to_string()));
    }
    fetch_with(&creds)
}

/// Agent 를 한 번만 만든다. 새로 만들면 rustls 설정이 매번 다시 구성된다.
fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            // 4xx/5xx 도 응답으로 받는다. 그래야 429 의 `retry-after` 를 읽을 수 있다.
            .http_status_as_error(false)
            .build()
            .new_agent()
    })
}

fn fetch_with(creds: &Credentials) -> Result<UsageResponse, FetchError> {
    let mut resp = agent()
        .get(ENDPOINT)
        .header("Authorization", &format!("Bearer {}", creds.access_token))
        .header("anthropic-beta", OAUTH_BETA)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .call()
        .map_err(|e| {
            FetchError::Other(anyhow::Error::new(e).context("usage 엔드포인트 호출 실패"))
        })?;

    let status = resp.status().as_u16();
    let retry_after = retry_after_secs(resp.headers());

    if status != 200 {
        return Err(status_error(status, retry_after));
    }

    let body = resp
        .body_mut()
        .read_to_string()
        .context("응답 본문을 읽지 못했습니다")
        .map_err(FetchError::Other)?;

    parse_body(&body).map_err(FetchError::Other)
}

/// `Retry-After` 는 초 단위. 0 이나 파싱 실패는 "값이 없음"으로 본다 —
/// 이 엔드포인트는 실제로 `retry-after: 0` 을 주면서 계속 막는 경우가 있어서,
/// 0 을 그대로 안내하면 "0초 뒤에 다시" 라는 틀린 말이 된다.
fn retry_after_secs(headers: &ureq::http::HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
}

fn status_error(status: u16, retry_after: Option<u64>) -> FetchError {
    match status {
        401 | 403 => FetchError::Unauthorized(auth::reauth_hint().to_string()),
        429 => FetchError::Other(match retry_after {
            Some(secs) => {
                anyhow::anyhow!("조회가 제한되었습니다 (HTTP 429). {secs}초 후 다시 시도하세요")
            }
            None => anyhow::anyhow!("조회가 제한되었습니다 (HTTP 429). 잠시 후 다시 시도하세요"),
        }),
        other => FetchError::Other(anyhow::anyhow!("서버가 HTTP {other} 를 반환했습니다")),
    }
}

fn parse_body(body: &str) -> Result<UsageResponse> {
    let parsed: UsageResponse = serde_json::from_str(body).context("usage 응답 파싱 실패")?;
    if parsed.limits.is_empty() {
        bail!("응답에 limits 항목이 없습니다 (스키마가 변경되었을 수 있습니다)");
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::usage::{LimitId, Severity};

    /// 실제 응답에서 발췌 — 최상위의 코드네임 필드들이 섞여 있어도
    /// limits 만 읽어서 동작해야 한다.
    const SAMPLE: &str = r#"{
      "five_hour": {"utilization": 52.0, "resets_at": "2026-08-18T12:29:59.748836+00:00"},
      "tangelo": null, "iguana_necktie": null,
      "nimbus_quill": {"utilization": 0.0, "resets_at": null},
      "limits": [
        {"kind":"session","group":"session","percent":52,"severity":"normal",
         "resets_at":"2026-08-18T12:29:59.748836+00:00","scope":null,"is_active":false},
        {"kind":"weekly_all","group":"weekly","percent":54,"severity":"normal",
         "resets_at":"2026-08-18T15:59:59.748863+00:00","scope":null,"is_active":false},
        {"kind":"weekly_scoped","group":"weekly","percent":73,"severity":"normal",
         "resets_at":"2026-08-18T15:59:59.749106+00:00",
         "scope":{"model":{"id":null,"display_name":"Fable"},"surface":null},"is_active":true}
      ]
    }"#;

    #[test]
    fn parses_real_response() {
        let limits = parse_body(SAMPLE).unwrap().limits;
        assert_eq!(limits.len(), 3);
        let normalized = super::super::model::to_limits(&limits);
        assert_eq!(
            normalized[0].window_duration,
            Some(chrono::TimeDelta::hours(5))
        );
        assert_eq!(
            normalized[1].window_duration,
            Some(chrono::TimeDelta::days(7))
        );
        assert_eq!(normalized[2].scope.as_deref(), Some("Fable"));
        assert!(limits[2].is_active);
        assert_eq!(limits[2].percent, 73.0);
    }

    /// 서버가 새 kind 를 추가해도 패닉 없이 표시되어야 한다.
    #[test]
    fn unknown_kind_is_still_rendered() {
        let body = r#"{"limits":[{"kind":"monthly_burst","percent":12,
            "scope":{"model":{"display_name":"Opus"}}}]}"#;
        let limits = parse_body(body).unwrap().limits;
        let normalized = super::super::model::to_limits(&limits);
        assert_eq!(normalized[0].id, LimitId::new("monthly_burst:Opus"));
        assert_eq!(normalized[0].severity, Severity::Normal);
    }

    /// severity 를 모를 때는 percent 로 보수적으로 판단한다.
    #[test]
    fn unknown_severity_falls_back_to_percent() {
        let body = r#"{"limits":[{"kind":"session","percent":93,"severity":"spicy"}]}"#;
        let limits = parse_body(body).unwrap().limits;
        assert_eq!(limits[0].severity(), Severity::Critical);
    }

    fn headers(pairs: &[(&str, &str)]) -> ureq::http::HeaderMap {
        let mut h = ureq::http::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                ureq::http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                ureq::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    /// 이 엔드포인트는 실제로 `retry-after: 0` 을 주면서 계속 막는다.
    /// 0 을 그대로 쓰면 "0초 후 다시 시도" 라는 틀린 안내가 된다.
    #[test]
    fn zero_retry_after_is_treated_as_missing() {
        assert_eq!(retry_after_secs(&headers(&[("retry-after", "0")])), None);
        assert_eq!(retry_after_secs(&headers(&[])), None);
        assert_eq!(retry_after_secs(&headers(&[("retry-after", "soon")])), None);
        assert_eq!(
            retry_after_secs(&headers(&[("retry-after", "30")])),
            Some(30)
        );
    }

    #[test]
    fn rate_limit_message_uses_retry_after_when_useful() {
        let with = status_error(429, Some(30)).to_string();
        assert!(with.contains("30초 후"), "{with}");

        let without = status_error(429, None).to_string();
        assert!(without.contains("잠시 후"), "{without}");
        assert!(!without.contains("0초"), "0 을 안내하면 안 된다: {without}");
    }

    #[test]
    fn auth_statuses_become_reauth_hints() {
        for code in [401, 403] {
            match status_error(code, None) {
                FetchError::Unauthorized(m) => assert!(m.contains("claude")),
                other => panic!("HTTP {code} 는 재인증 안내여야 함: {other:?}"),
            }
        }
    }

    #[test]
    fn empty_limits_is_an_error() {
        assert!(parse_body(r#"{"limits":[]}"#).is_err());
    }
}
