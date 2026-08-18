//! Codex 한도 조회 진입점.
//!
//! `account/rateLimits/read` 는 호출 제한이 없고 app-server 가 값을 들고 있어
//! 매번 직접 조회해도 무리가 없다. 그래서 ccmeter 와 달리 캐시 계층이 없다.

use super::{client, model};
use crate::FetchError;
use crate::meter::Snapshot;

pub fn fetch(tz: &str) -> Result<Snapshot, FetchError> {
    client::fetch().map(|resp| Snapshot::live(model::to_meters(&resp, tz)))
}
