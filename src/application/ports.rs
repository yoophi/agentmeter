//! 애플리케이션이 외부 세계에 요구하는 포트.

use crate::domain::usage::UsageSnapshot;

use super::{FetchError, Settings};

/// 한 번의 사용량 조회 조건.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchPolicy {
    PreferCached,
    Fresh,
}

/// Claude/Codex 같은 외부 사용량 공급자가 구현하는 아웃바운드 포트.
pub trait UsageSource: Send + Sync {
    fn fetch(&self, policy: FetchPolicy) -> Result<UsageSnapshot, FetchError>;
}

/// 설정 저장소가 구현하는 아웃바운드 포트.
///
/// 파일이 없으면 `None`을 돌려주고 기본값 선택은 애플리케이션에 맡긴다.
pub trait SettingsRepository: Send + Sync {
    fn load(&self) -> anyhow::Result<Option<Settings>>;
    fn save(&self, settings: &Settings) -> anyhow::Result<()>;
}
