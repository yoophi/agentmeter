//! GLM 계정 쿼터를 application port 에 연결하고 짧은 polling 을 흡수한다.

use std::sync::Mutex;

use chrono::TimeDelta;

use super::{client, model};
use crate::application::{FetchError, FetchPolicy, UsageSource};
use crate::domain::usage::{Origin, UsageSnapshot};

/// 원격 호출이고 문서화되지 않은 API 다. 화면 갱신 주기보다 넉넉히 잡는다.
const CACHE_TTL: TimeDelta = TimeDelta::minutes(1);

#[derive(Debug, Default)]
pub struct GlmUsageSource {
    cached: Mutex<Option<UsageSnapshot>>,
}

impl UsageSource for GlmUsageSource {
    fn fetch(&self, policy: FetchPolicy) -> Result<UsageSnapshot, FetchError> {
        let now = chrono::Local::now();
        if policy == FetchPolicy::PreferCached
            && let Some(snapshot) = self.fresh_cache(now)
        {
            return Ok(snapshot);
        }

        let limits = match self.load() {
            Ok(limits) => limits,
            // 직전 값이 있으면 실패했다고 화면을 비우지 않는다.
            Err(error) if policy == FetchPolicy::PreferCached => {
                return match self.stale_cache() {
                    Some(snapshot) => Ok(snapshot),
                    None => Err(error),
                };
            }
            Err(error) => return Err(error),
        };

        let snapshot = UsageSnapshot::live(limits, now);
        *self.cache() = Some(snapshot.clone());
        Ok(snapshot)
    }
}

impl GlmUsageSource {
    fn cache(&self) -> std::sync::MutexGuard<'_, Option<UsageSnapshot>> {
        self.cached
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn fresh_cache(&self, now: chrono::DateTime<chrono::Local>) -> Option<UsageSnapshot> {
        let cached = self.cache();
        let snapshot = cached.as_ref()?;
        if now - snapshot.origin.at >= CACHE_TTL {
            return None;
        }
        let mut snapshot = snapshot.clone();
        snapshot.origin = Origin::cache(snapshot.origin.at, false);
        Some(snapshot)
    }

    fn stale_cache(&self) -> Option<UsageSnapshot> {
        let cached = self.cache();
        let snapshot = cached.as_ref()?;
        let mut snapshot = snapshot.clone();
        snapshot.origin = Origin::cache(snapshot.origin.at, true);
        Some(snapshot)
    }

    fn load(&self) -> Result<Vec<crate::domain::usage::UsageLimit>, FetchError> {
        let envelope = client::fetch()?;
        let data = envelope
            .data
            .ok_or_else(|| FetchError::Other(anyhow::anyhow!("the response has no data")))?;
        let limits = model::to_limits(&data);
        if limits.is_empty() {
            return Err(FetchError::Other(anyhow::anyhow!(
                "no known limit types (the schema may have changed)"
            )));
        }
        Ok(limits)
    }
}
