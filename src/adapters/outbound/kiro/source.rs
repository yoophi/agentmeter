//! Kiro CLI 사용량을 application port에 연결하고 짧은 polling을 흡수한다.

use std::sync::Mutex;

use chrono::TimeDelta;

use super::{client, model};
use crate::application::{FetchError, FetchPolicy, UsageSource};
use crate::domain::usage::{Origin, UsageSnapshot};

const CACHE_TTL: TimeDelta = TimeDelta::minutes(5);

#[derive(Debug, Default)]
pub struct KiroUsageSource {
    cached: Mutex<Option<UsageSnapshot>>,
}

impl UsageSource for KiroUsageSource {
    fn fetch(&self, policy: FetchPolicy) -> Result<UsageSnapshot, FetchError> {
        let now = chrono::Local::now();
        if policy == FetchPolicy::PreferCached {
            let cached = self
                .cached
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(snapshot) = cached.as_ref()
                && now - snapshot.origin.at < CACHE_TTL
            {
                let mut snapshot = snapshot.clone();
                snapshot.origin = Origin::cache(snapshot.origin.at, false);
                return Ok(snapshot);
            }
        }

        let fetched = (|| {
            let raw = client::fetch()?;
            let usage = model::parse(&raw)
                .map_err(|error| FetchError::Other(error.context("Kiro usage 출력 파싱 실패")))?;
            model::to_limit(&usage)
                .map_err(|error| FetchError::Other(error.context("Kiro usage 변환 실패")))
        })();
        let limit = match fetched {
            Ok(limit) => limit,
            Err(error) if policy == FetchPolicy::PreferCached => {
                let cached = self
                    .cached
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(snapshot) = cached.as_ref() {
                    let mut snapshot = snapshot.clone();
                    snapshot.origin = Origin::cache(snapshot.origin.at, true);
                    return Ok(snapshot);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let snapshot = UsageSnapshot::live(vec![limit], now);
        *self
            .cached
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(snapshot.clone());
        Ok(snapshot)
    }
}
