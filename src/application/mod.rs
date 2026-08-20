//! 외부 기술에 의존하지 않는 애플리케이션 사용 사례와 포트.

mod error;
mod ports;
mod refresh;
mod session;
mod settings;
mod usage;
mod watch;

pub(crate) use error::FetchError;
pub(crate) use ports::{
    FetchPolicy, HistoryRepository, HistoryRestore, SettingsRepository, UsageSource, WindowHistory,
};
pub(crate) use refresh::{RefreshCoordinator, RefreshDecision};
pub(crate) use session::{LiveSession, SessionState};
pub(crate) use settings::{Settings, SettingsApplication};
pub(crate) use usage::{AgentInfo, AgentResult, RegisteredAgent, UsageApplication};
pub(crate) use watch::{UsageSample, WatchPane, WatchState};
