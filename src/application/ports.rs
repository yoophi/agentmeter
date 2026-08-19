//! 애플리케이션이 외부 세계에 요구하는 포트.

use std::collections::BTreeMap;

use chrono::{DateTime, Local};

use crate::domain::usage::{LimitId, UsageSnapshot, UsageWindow};

use super::{FetchError, Settings, UsageSample};

#[derive(Debug, Clone)]
pub struct WindowHistory {
    pub window: UsageWindow,
    pub series: BTreeMap<LimitId, Vec<UsageSample>>,
}

#[derive(Debug, Clone, Default)]
pub struct HistoryRestore {
    pub snapshot: Option<UsageSnapshot>,
    pub windows: Vec<WindowHistory>,
    pub warnings: Vec<String>,
}

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

/// 한도 창별 표본을 파일 같은 외부 저장소에 보존하는 포트.
pub trait HistoryRepository: Send + Sync {
    /// 유효한 활성 창은 모두 복원하고, 손상된 파일은 warnings로 격리한다.
    fn restore_active(&self, provider: &str, at: DateTime<Local>)
    -> anyhow::Result<HistoryRestore>;

    /// snapshot을 해당 창 파일에 append하고 갱신된 창 이력을 돌려준다.
    fn record(
        &self,
        provider: &str,
        snapshot: &UsageSnapshot,
    ) -> anyhow::Result<Vec<WindowHistory>>;
}
