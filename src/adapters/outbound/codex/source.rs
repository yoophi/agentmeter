//! Codex 한도 조회 진입점.
//!
//! `account/rateLimits/read` 는 호출 제한이 없고 app-server 가 값을 들고 있어
//! 매번 직접 조회해도 무리가 없다. 그래서 ccmeter 와 달리 캐시 계층이 없다.

use super::{client, model};
use crate::application::FetchError;
use crate::application::{FetchPolicy, UsageSource};
use crate::domain::usage::UsageSnapshot;

/// Codex app-server를 사용하는 `UsageSource` 어댑터.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodexUsageSource;

impl UsageSource for CodexUsageSource {
    fn fetch(&self, _policy: FetchPolicy) -> Result<UsageSnapshot, FetchError> {
        fetch()
    }
}

pub fn fetch() -> Result<UsageSnapshot, FetchError> {
    client::fetch()
        .map(|response| UsageSnapshot::live(model::to_limits(&response), chrono::Local::now()))
}
