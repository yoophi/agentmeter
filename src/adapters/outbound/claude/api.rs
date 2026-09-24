//! `/api/oauth/usage` 호출.

use anyhow::{Context, Result, bail};
use std::time::Duration;

use super::auth::{self, Credentials};
use super::model::UsageResponse;
use crate::application::FetchError;

/// `cedar_ember=1` 이어야 세션 리셋 권한 블록이 채워진다 (Claude Code 도 이렇게 부른다).
/// `skip_spend=1` 은 쓰지 않는 `spend` 블록을 빼 달라는 뜻이다.
const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage?cedar_ember=1&skip_spend=1";
const OAUTH_BETA: &str = "oauth-2025-04-20";
/// `claude --version` 을 읽지 못했을 때 쓰는 버전. 이 기능을 확인한 Claude Code 버전이다.
const FALLBACK_CLI_VERSION: &str = "2.1.280";
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
        // 서버는 이 두 헤더로 요청 경로(surface)를 판정한다. CLI 로 보이지 않으면
        // `cedar_ember` 가 `ineligible_reason: "surface"` 로 비어서 온다.
        .header("User-Agent", user_agent())
        .header("x-app", "cli")
        .header("Accept", "application/json")
        .call()
        .map_err(|e| {
            FetchError::Other(anyhow::Error::new(e).context("the usage endpoint call failed"))
        })?;

    let status = resp.status().as_u16();
    let retry_after = retry_after_secs(resp.headers());

    if status != 200 {
        return Err(status_error(status, retry_after));
    }

    let body = resp
        .body_mut()
        .read_to_string()
        .context("could not read the response body")
        .map_err(FetchError::Other)?;

    parse_body(&body).map_err(FetchError::Other)
}

/// Claude Code CLI 와 같은 형태의 User-Agent. 버전은 프로세스당 한 번만 읽는다.
fn user_agent() -> &'static str {
    static UA: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    UA.get_or_init(|| {
        let version = installed_cli_version().unwrap_or_else(|| FALLBACK_CLI_VERSION.to_string());
        format!("claude-cli/{version} (external, cli)")
    })
}

fn installed_cli_version() -> Option<String> {
    let out = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_cli_version(&String::from_utf8_lossy(&out.stdout))
}

/// `2.1.280 (Claude Code)` → `2.1.280`
fn parse_cli_version(raw: &str) -> Option<String> {
    let version = raw.split_whitespace().next()?;
    let valid = !version.is_empty()
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    valid.then(|| version.to_string())
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
                anyhow::anyhow!("rate limited (HTTP 429). retry in {secs}s")
            }
            None => anyhow::anyhow!("rate limited (HTTP 429). retry shortly"),
        }),
        other => FetchError::Other(anyhow::anyhow!("the server returned HTTP {other}")),
    }
}

fn parse_body(body: &str) -> Result<UsageResponse> {
    let parsed: UsageResponse =
        serde_json::from_str(body).context("could not parse the usage response")?;
    if parsed.limits.is_empty() {
        bail!("the response has no limits (the schema may have changed)");
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
    fn cli_version_is_read_from_the_version_line() {
        assert_eq!(
            parse_cli_version("2.1.280 (Claude Code)\n").as_deref(),
            Some("2.1.280")
        );
        assert_eq!(parse_cli_version(""), None);
        assert_eq!(parse_cli_version("command not found"), None);
        assert_eq!(parse_cli_version("2.1. (Claude Code)"), None);
    }

    #[test]
    fn rate_limit_message_uses_retry_after_when_useful() {
        let with = status_error(429, Some(30)).to_string();
        assert!(with.contains("retry in 30s"), "{with}");

        let without = status_error(429, None).to_string();
        assert!(without.contains("retry shortly"), "{without}");
        assert!(!without.contains("0s"), "0 을 안내하면 안 된다: {without}");
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
