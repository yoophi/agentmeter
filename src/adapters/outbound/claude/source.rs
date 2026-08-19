//! Claude 사용량 획득 정책.
//!
//! 캐시·HTTP·시계를 작은 내부 포트로 감싸 정책 자체를 외부 기술과 분리한다.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use super::api;
use super::model::{UsageResponse, to_limits};
use crate::application::{FetchError, FetchPolicy, UsageSource};
use crate::domain::usage::{Origin, UsageSnapshot};

/// 이보다 오래된 캐시는 직접 조회로 갱신을 시도한다.
pub const MAX_AGE: Duration = Duration::from_secs(15 * 60);
/// 직접 조회가 실패한 뒤 다시 시도하기까지 기다리는 시간.
pub const NEG_TTL: Duration = Duration::from_secs(5 * 60);

const CACHE_REL: &str = ".claude/token-scope-oauth-usage.json";
const AGENT_CACHE_REL: &str = ".cache/agentmeter/claude-usage.json";
const FAIL_MARKER_REL: &str = ".cache/agentmeter/claude-usage.err";

#[derive(Debug, Deserialize)]
struct CacheFile {
    captured_at: String,
    usage: UsageResponse,
}

#[derive(Serialize)]
struct CacheFileRef<'a> {
    captured_at: String,
    usage: &'a UsageResponse,
}

trait UsageClient: Send + Sync {
    fn fetch(&self) -> Result<UsageResponse, FetchError>;
}

trait CacheStore: Send + Sync {
    fn read(&self) -> Option<(DateTime<Local>, UsageResponse)>;
    fn write(&self, at: DateTime<Local>, response: &UsageResponse) -> anyhow::Result<()>;
    fn backoff_active(&self, now: SystemTime) -> bool;
    fn mark_failure(&self);
    fn clear_backoff(&self);
}

trait Clock: Send + Sync {
    fn local_now(&self) -> DateTime<Local>;
    fn system_now(&self) -> SystemTime;
}

struct ApiClient;

impl UsageClient for ApiClient {
    fn fetch(&self) -> Result<UsageResponse, FetchError> {
        api::fetch_response()
    }
}

struct FileCacheStore;

impl CacheStore for FileCacheStore {
    fn read(&self) -> Option<(DateTime<Local>, UsageResponse)> {
        [CACHE_REL, AGENT_CACHE_REL]
            .into_iter()
            .filter_map(home_path)
            .filter_map(|path| read_cache_at(&path))
            .max_by_key(|(at, _)| *at)
    }

    fn write(&self, at: DateTime<Local>, response: &UsageResponse) -> anyhow::Result<()> {
        let path = home_path(AGENT_CACHE_REL).context("HOME 을 찾을 수 없습니다")?;
        write_cache_at(&path, at, response)
    }

    fn backoff_active(&self, now: SystemTime) -> bool {
        let Some(path) = home_path(FAIL_MARKER_REL) else {
            return false;
        };
        let Ok(modified) = std::fs::metadata(path).and_then(|metadata| metadata.modified()) else {
            return false;
        };
        now.duration_since(modified)
            .map(|age| age < NEG_TTL)
            .unwrap_or(false)
    }

    fn mark_failure(&self) {
        let Some(path) = home_path(FAIL_MARKER_REL) else {
            return;
        };
        if let Some(directory) = path.parent() {
            let _ = std::fs::create_dir_all(directory);
        }
        let _ = std::fs::write(path, b"");
    }

    fn clear_backoff(&self) {
        if let Some(path) = home_path(FAIL_MARKER_REL) {
            let _ = std::fs::remove_file(path);
        }
    }
}

struct SystemClock;

impl Clock for SystemClock {
    fn local_now(&self) -> DateTime<Local> {
        Local::now()
    }

    fn system_now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Claude Code 캐시/API를 사용하는 아웃바운드 어댑터.
#[derive(Clone)]
pub struct ClaudeUsageSource {
    client: Arc<dyn UsageClient>,
    cache: Arc<dyn CacheStore>,
    clock: Arc<dyn Clock>,
}

impl Default for ClaudeUsageSource {
    fn default() -> Self {
        Self {
            client: Arc::new(ApiClient),
            cache: Arc::new(FileCacheStore),
            clock: Arc::new(SystemClock),
        }
    }
}

impl UsageSource for ClaudeUsageSource {
    fn fetch(&self, policy: FetchPolicy) -> Result<UsageSnapshot, FetchError> {
        match policy {
            FetchPolicy::Fresh => self.fetch_live(),
            FetchPolicy::PreferCached => self.fetch_prefer_cached(),
        }
    }
}

impl ClaudeUsageSource {
    fn fetch_prefer_cached(&self) -> Result<UsageSnapshot, FetchError> {
        let now = self.clock.local_now();
        let cached = self.cache.read();

        if let Some((captured_at, response)) = &cached
            && !is_stale(*captured_at, now)
        {
            return Ok(snapshot(response, Origin::cache(*captured_at, false)));
        }

        let refreshed = if self.cache.backoff_active(self.clock.system_now()) {
            Err(FetchError::Other(anyhow::anyhow!(
                "조회가 제한되어 잠시 쉬는 중입니다. {}분 뒤 다시 시도합니다",
                NEG_TTL.as_secs() / 60
            )))
        } else {
            self.fetch_live()
        };

        refreshed.or_else(|error| match cached {
            Some((captured_at, response)) => {
                Ok(snapshot(&response, Origin::cache(captured_at, true)))
            }
            None => Err(error),
        })
    }

    fn fetch_live(&self) -> Result<UsageSnapshot, FetchError> {
        match self.client.fetch() {
            Ok(response) => {
                let captured_at = self.clock.local_now();
                // Claude Code 자체 캐시가 갱신되지 않아도 방금 실측한 값은 보존한다.
                let _ = self.cache.write(captured_at, &response);
                self.cache.clear_backoff();
                Ok(UsageSnapshot::live(
                    to_limits(&response.limits),
                    captured_at,
                ))
            }
            Err(error) => {
                if !matches!(error, FetchError::Unauthorized(_)) {
                    self.cache.mark_failure();
                }
                Err(error)
            }
        }
    }

    #[cfg(test)]
    fn with_dependencies(
        client: Arc<dyn UsageClient>,
        cache: Arc<dyn CacheStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            client,
            cache,
            clock,
        }
    }
}

fn snapshot(response: &UsageResponse, origin: Origin) -> UsageSnapshot {
    UsageSnapshot {
        limits: to_limits(&response.limits),
        origin,
    }
}

fn is_stale(captured_at: DateTime<Local>, now: DateTime<Local>) -> bool {
    match (now - captured_at).to_std() {
        Ok(age) => age > MAX_AGE,
        // 미래 시각이면 갓 쓰인 값으로 본다.
        Err(_) => false,
    }
}

fn home_path(relative: &str) -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(relative))
}

fn parse_cache(raw: &str) -> anyhow::Result<(DateTime<Local>, UsageResponse)> {
    let file: CacheFile = serde_json::from_str(raw).context("usage 캐시 파싱 실패")?;
    let captured_at = DateTime::parse_from_rfc3339(&file.captured_at)
        .context("captured_at 을 읽을 수 없습니다")?
        .with_timezone(&Local);
    Ok((captured_at, file.usage))
}

fn read_cache_at(path: &std::path::Path) -> Option<(DateTime<Local>, UsageResponse)> {
    parse_cache(&std::fs::read_to_string(path).ok()?).ok()
}

fn write_cache_at(
    path: &std::path::Path,
    at: DateTime<Local>,
    response: &UsageResponse,
) -> anyhow::Result<()> {
    let parent = path.parent().context("캐시 디렉터리를 찾을 수 없습니다")?;
    std::fs::create_dir_all(parent).context("캐시 디렉터리를 만들지 못했습니다")?;
    let bytes = serde_json::to_vec(&CacheFileRef {
        captured_at: at.to_rfc3339(),
        usage: response,
    })
    .context("usage 캐시 직렬화 실패")?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, bytes).context("usage 임시 캐시 저장 실패")?;
    std::fs::rename(&temporary, path).context("usage 캐시 교체 실패")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use chrono::{TimeDelta, TimeZone};

    use super::*;
    use crate::domain::usage::{OriginKind, Severity};

    const SAMPLE: &str = r#"{
      "captured_at": "2026-08-18T11:54:53.330Z",
      "usage": {
        "limits": [
          {"kind":"session","percent":65,"severity":"normal",
           "resets_at":"2026-08-18T12:29:59.829638+00:00","scope":null,"is_active":false},
          {"kind":"weekly_scoped","percent":75,"severity":"warning",
           "resets_at":"2026-08-18T15:59:59.829940+00:00",
           "scope":{"model":{"display_name":"Fable"}},"is_active":true}
        ]
      }
    }"#;

    fn at(hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 8, 18, hour, minute, 0)
            .single()
            .unwrap()
    }

    fn response() -> UsageResponse {
        parse_cache(SAMPLE).unwrap().1
    }

    struct FakeClient {
        calls: AtomicUsize,
        fail: bool,
    }

    impl UsageClient for FakeClient {
        fn fetch(&self) -> Result<UsageResponse, FetchError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(FetchError::Other(anyhow::anyhow!("network failed")))
            } else {
                Ok(response())
            }
        }
    }

    struct FakeCache {
        cached: Mutex<Option<(DateTime<Local>, UsageResponse)>>,
        backoff: bool,
        marked: AtomicBool,
        cleared: AtomicBool,
    }

    impl CacheStore for FakeCache {
        fn read(&self) -> Option<(DateTime<Local>, UsageResponse)> {
            self.cached.lock().unwrap().clone()
        }

        fn write(&self, at: DateTime<Local>, response: &UsageResponse) -> anyhow::Result<()> {
            *self.cached.lock().unwrap() = Some((at, response.clone()));
            Ok(())
        }

        fn backoff_active(&self, _now: SystemTime) -> bool {
            self.backoff
        }

        fn mark_failure(&self) {
            self.marked.store(true, Ordering::SeqCst);
        }

        fn clear_backoff(&self) {
            self.cleared.store(true, Ordering::SeqCst);
        }
    }

    struct FakeClock(DateTime<Local>);

    impl Clock for FakeClock {
        fn local_now(&self) -> DateTime<Local> {
            self.0
        }

        fn system_now(&self) -> SystemTime {
            SystemTime::UNIX_EPOCH
        }
    }

    fn source(
        client: Arc<FakeClient>,
        cache: Arc<FakeCache>,
        now: DateTime<Local>,
    ) -> ClaudeUsageSource {
        ClaudeUsageSource::with_dependencies(client, cache, Arc::new(FakeClock(now)))
    }

    #[test]
    fn parses_the_cache_file() {
        let (captured_at, response) = parse_cache(SAMPLE).unwrap();
        assert_eq!(
            captured_at.to_utc().to_rfc3339(),
            "2026-08-18T11:54:53.330+00:00"
        );
        assert_eq!(response.limits.len(), 2);
    }

    #[test]
    fn fresh_cache_avoids_the_client_through_the_public_port() {
        let client = Arc::new(FakeClient {
            calls: AtomicUsize::new(0),
            fail: false,
        });
        let cache = Arc::new(FakeCache {
            cached: Mutex::new(Some((at(12, 0), response()))),
            backoff: false,
            marked: AtomicBool::new(false),
            cleared: AtomicBool::new(false),
        });
        let snapshot = source(client.clone(), cache, at(12, 1))
            .fetch(FetchPolicy::PreferCached)
            .unwrap();

        assert_eq!(client.calls.load(Ordering::SeqCst), 0);
        assert_eq!(snapshot.origin.kind, OriginKind::Cache);
        assert_eq!(snapshot.limits[1].severity, Severity::Warning);
        assert!(snapshot.limits[1].active);
    }

    #[test]
    fn stale_cache_is_retained_when_refresh_fails() {
        let client = Arc::new(FakeClient {
            calls: AtomicUsize::new(0),
            fail: true,
        });
        let cache = Arc::new(FakeCache {
            cached: Mutex::new(Some((at(11, 0), response()))),
            backoff: false,
            marked: AtomicBool::new(false),
            cleared: AtomicBool::new(false),
        });
        let snapshot = source(client.clone(), cache.clone(), at(12, 0))
            .fetch(FetchPolicy::PreferCached)
            .unwrap();

        assert_eq!(client.calls.load(Ordering::SeqCst), 1);
        assert!(cache.marked.load(Ordering::SeqCst));
        assert!(snapshot.origin.refresh_failed);
        assert_eq!(snapshot.origin.kind, OriginKind::Cache);
    }

    #[test]
    fn fresh_policy_bypasses_cache_and_clears_backoff() {
        let client = Arc::new(FakeClient {
            calls: AtomicUsize::new(0),
            fail: false,
        });
        let cache = Arc::new(FakeCache {
            cached: Mutex::new(Some((at(12, 0), response()))),
            backoff: true,
            marked: AtomicBool::new(false),
            cleared: AtomicBool::new(false),
        });
        let snapshot = source(client.clone(), cache.clone(), at(12, 1))
            .fetch(FetchPolicy::Fresh)
            .unwrap();

        assert_eq!(client.calls.load(Ordering::SeqCst), 1);
        assert!(cache.cleared.load(Ordering::SeqCst));
        assert_eq!(snapshot.origin.kind, OriginKind::Live);
    }

    #[test]
    fn limit_without_reset_keeps_its_window_duration() {
        let raw = r#"{"captured_at":"2026-08-19T01:00:00Z","usage":{"limits":[
            {"kind":"session","percent":0,"severity":"normal","resets_at":null,
             "scope":null,"is_active":false}]}}"#;
        let limits = to_limits(&parse_cache(raw).unwrap().1.limits);
        assert_eq!(limits[0].window_duration, Some(TimeDelta::hours(5)));
        assert!(limits[0].resets_at.is_none());
    }

    #[test]
    fn staleness_uses_the_supplied_clock() {
        assert!(!is_stale(at(12, 0), at(12, 1)));
        assert!(is_stale(at(12, 0), at(12, 16)));
        assert!(!is_stale(at(13, 0), at(12, 0)));
    }

    #[test]
    fn broken_cache_is_ignored() {
        assert!(parse_cache("{}").is_err());
        assert!(parse_cache(r#"{"captured_at":"nope","usage":{"limits":[]}}"#).is_err());
    }

    #[test]
    fn live_response_round_trips_through_agent_cache() {
        let at = at(12, 33);
        let path = std::env::temp_dir().join(format!(
            "agentmeter-claude-cache-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        write_cache_at(&path, at, &response()).unwrap();
        let (loaded_at, loaded) = read_cache_at(&path).unwrap();
        let _ = std::fs::remove_file(path);
        assert_eq!(loaded_at, at);
        assert_eq!(loaded.limits.len(), 2);
    }
}
