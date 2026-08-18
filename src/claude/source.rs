//! 한도 값을 어디서 가져올지 정한다.
//!
//! Claude Code 는 `/api/oauth/usage` 응답을 통째로
//! `~/.claude/token-scope-oauth-usage.json` 에 캐시해 둔다.
//! 그 파일을 읽으면 네트워크 호출이 없으므로 즉시 응답하고 `HTTP 429` 도 없다.
//!
//! 다만 Claude Code 가 갱신해 줄 때까지 값이 멈춰 있고, 우리가 갱신을 유도할
//! 방법은 없다 — `claude -p` 로 추론 요청을 보내도 이 파일은 그대로였다.
//! 그래서 캐시가 [`MAX_AGE`] 보다 오래되면 그때만 직접 호출한다.
//!
//! 직접 호출이 실패하면 [`NEG_TTL`] 동안 다시 시도하지 않는다.
//! 이 엔드포인트는 자주 부르면 `429` 를 돌려주는데, 막힌 상태에서 계속 두드리면
//! 제한이 풀리는 것만 늦춘다.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use chrono::{DateTime, Local, Utc};
use serde::Deserialize;

use super::api;
use super::model::{UsageResponse, to_meters};
use crate::FetchError;
use crate::meter::{Origin, Snapshot};

/// 이보다 오래된 캐시는 직접 조회로 갱신을 시도한다.
/// 세션 한도는 5시간, 주간 한도는 7일 창이라 몇 분 차이는 화면에 드러나지 않는다.
pub const MAX_AGE: Duration = Duration::from_secs(15 * 60);

/// 직접 조회가 실패한 뒤 다시 시도하기까지 기다리는 시간.
pub const NEG_TTL: Duration = Duration::from_secs(5 * 60);

const CACHE_REL: &str = ".claude/token-scope-oauth-usage.json";
const FAIL_MARKER_REL: &str = ".cache/agentmeter/claude-usage.err";

#[derive(Debug, Deserialize)]
struct CacheFile {
    captured_at: String,
    /// `/api/oauth/usage` 응답이 그대로 들어 있다.
    usage: UsageResponse,
}

fn home_path(rel: &str) -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(rel))
}

/// 캐시를 먼저 보고, 없거나 오래됐으면 직접 조회한다.
///
/// 조회에 실패해도 캐시가 있으면 그 값을 돌려준다 — 낡은 값이라도 보여주는 편이
/// 빈 화면보다 낫다. 대신 얼마나 낡았는지와 갱신 실패 사실을 함께 표시한다.
pub fn fetch(tz: &str) -> Result<Snapshot, FetchError> {
    let cached = read_cache();

    if let Some((at, resp)) = &cached
        && !is_stale(*at)
    {
        return Ok(snapshot(resp, tz, Origin::cache(*at, false)));
    }

    // 최근에 실패했다면 다시 두드리지 않는다
    let refreshed = if backoff_active() {
        Err(FetchError::Other(anyhow::anyhow!(
            "조회가 제한되어 잠시 쉬는 중입니다. {}분 뒤 다시 시도합니다",
            NEG_TTL.as_secs() / 60
        )))
    } else {
        fetch_live(tz)
    };

    refreshed.or_else(|e| match cached {
        Some((at, resp)) => Ok(snapshot(&resp, tz, Origin::cache(at, true))),
        None => Err(e),
    })
}

/// 캐시를 건너뛰고 항상 직접 조회한다 (`--live`).
///
/// 실패하면 백오프를 건다. 재인증 오류는 예외 — 사용자가 로그인하면
/// 즉시 반영되어야 한다.
pub fn fetch_live(tz: &str) -> Result<Snapshot, FetchError> {
    match api::fetch_response() {
        Ok(resp) => {
            clear_backoff();
            Ok(Snapshot::live(to_meters(&resp.limits, tz)))
        }
        Err(e) => {
            if !matches!(e, FetchError::Unauthorized(_)) {
                mark_failure();
            }
            Err(e)
        }
    }
}

fn snapshot(resp: &UsageResponse, tz: &str, origin: Origin) -> Snapshot {
    Snapshot {
        meters: to_meters(&resp.limits, tz),
        origin,
    }
}

fn is_stale(at: DateTime<Local>) -> bool {
    match (Utc::now() - at.with_timezone(&Utc)).to_std() {
        Ok(age) => age > MAX_AGE,
        // 미래 시각이면 음수가 되어 변환에 실패한다. 갓 쓰인 값으로 본다.
        Err(_) => false,
    }
}

fn read_cache() -> Option<(DateTime<Local>, UsageResponse)> {
    parse_cache(&std::fs::read_to_string(home_path(CACHE_REL)?).ok()?).ok()
}

fn parse_cache(raw: &str) -> anyhow::Result<(DateTime<Local>, UsageResponse)> {
    let file: CacheFile = serde_json::from_str(raw).context("usage 캐시 파싱 실패")?;
    let at = DateTime::parse_from_rfc3339(&file.captured_at)
        .context("captured_at 을 읽을 수 없습니다")?
        .with_timezone(&Local);
    Ok((at, file.usage))
}

// --- 실패 백오프 -------------------------------------------------------------
// `watch` 처럼 프로세스가 매번 새로 뜨는 경우에도 유지되어야 하므로
// 메모리가 아니라 파일의 수정 시각으로 기록한다.

fn backoff_active() -> bool {
    let Some(path) = home_path(FAIL_MARKER_REL) else {
        return false;
    };
    let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age < NEG_TTL)
        .unwrap_or(false)
}

fn mark_failure() {
    let Some(path) = home_path(FAIL_MARKER_REL) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // 내용은 쓰지 않는다 — 수정 시각만 있으면 된다
    let _ = std::fs::write(&path, b"");
}

fn clear_backoff() {
    if let Some(path) = home_path(FAIL_MARKER_REL) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meter::Level;

    /// 실제 `~/.claude/token-scope-oauth-usage.json` 에서 발췌.
    /// `usage` 안에 API 응답이 그대로 들어 있어 같은 타입으로 파싱된다.
    const SAMPLE: &str = r#"{
      "captured_at": "2026-08-18T11:54:53.330Z",
      "plan_name": "Pro",
      "usage": {
        "five_hour": {"utilization": 65, "resets_at": "2026-08-18T12:29:59.829638+00:00"},
        "tangelo": null,
        "limits": [
          {"kind":"session","group":"session","percent":65,"severity":"normal",
           "resets_at":"2026-08-18T12:29:59.829638+00:00","scope":null,"is_active":false},
          {"kind":"weekly_scoped","group":"weekly","percent":75,"severity":"warning",
           "resets_at":"2026-08-18T15:59:59.829940+00:00",
           "scope":{"model":{"display_name":"Fable","id":null}},"is_active":true}
        ]
      }
    }"#;

    #[test]
    fn parses_the_cache_file() {
        let (at, resp) = parse_cache(SAMPLE).unwrap();
        assert_eq!(at.to_utc().to_rfc3339(), "2026-08-18T11:54:53.330+00:00");
        assert_eq!(resp.limits.len(), 2);
        assert_eq!(resp.limits[0].percent, 65.0);
    }

    /// 캐시에서 읽은 값도 직접 조회와 똑같이 렌더링되어야 한다.
    #[test]
    fn cache_produces_the_same_meters() {
        let (at, resp) = parse_cache(SAMPLE).unwrap();
        let snap = snapshot(&resp, "Asia/Seoul", Origin::cache(at, false));
        assert_eq!(snap.meters[0].title, "Current session");
        assert_eq!(snap.meters[0].usage.label, "65% used");
        assert_eq!(snap.meters[1].title, "Current week (Fable)");
        assert!(snap.meters[1].emphasized, "is_active 는 강조로 이어진다");
        assert_eq!(snap.origin.kind, crate::meter::OriginKind::Cache);
    }

    /// 서버가 실제로 보내는 `severity: "warning"` 이 색 등급으로 이어지는지.
    #[test]
    fn warning_severity_is_honored() {
        let (_, resp) = parse_cache(SAMPLE).unwrap();
        let meters = to_meters(&resp.limits, "Asia/Seoul");
        assert_eq!(meters[1].usage.level, Level::Warning);
    }

    #[test]
    fn staleness_is_measured_against_max_age() {
        assert!(!is_stale(Local::now() - chrono::Duration::minutes(1)));
        assert!(is_stale(Local::now() - chrono::Duration::minutes(16)));
    }

    /// 시계가 어긋나 미래 시각이 적혀 있어도 패닉하거나 매번 조회하지 않는다.
    #[test]
    fn future_timestamp_is_not_stale() {
        assert!(!is_stale(Local::now() + chrono::Duration::hours(1)));
    }

    #[test]
    fn broken_cache_is_ignored() {
        assert!(parse_cache("{}").is_err());
        assert!(parse_cache(r#"{"captured_at":"nope","usage":{"limits":[]}}"#).is_err());
    }

    /// 낡은 캐시를 쓰는 중이면 그 사실이 화면 문구에 드러나야 한다.
    #[test]
    fn stale_cache_is_labeled_as_failed_refresh() {
        let (at, resp) = parse_cache(SAMPLE).unwrap();
        let snap = snapshot(&resp, "Asia/Seoul", Origin::cache(at, true));
        let text = snap.origin.text();
        assert!(text.contains("로컬 캐시"), "{text}");
        assert!(text.contains("갱신 실패"), "{text}");
    }
}
