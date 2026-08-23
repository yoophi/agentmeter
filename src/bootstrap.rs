//! 실제 포트 구현을 애플리케이션에 연결하는 조립 지점(composition root).

use std::path::PathBuf;
use std::sync::Arc;

use crate::adapters::outbound::claude::source::ClaudeUsageSource;
use crate::adapters::outbound::codex::source::CodexUsageSource;
use crate::adapters::outbound::config::FileSettingsRepository;
use crate::adapters::outbound::history::FileHistoryRepository;
use crate::adapters::outbound::kiro::source::KiroUsageSource;
use crate::application::{
    AgentInfo, HistoryRepository, RegisteredAgent, SettingsApplication, UsageApplication,
};

pub(crate) struct Runtime {
    pub(crate) usage: UsageApplication,
    pub(crate) settings: SettingsApplication,
    pub(crate) settings_path: PathBuf,
    pub(crate) history: Arc<dyn HistoryRepository>,
}

/// 프로덕션 어댑터는 오직 이곳에서 애플리케이션 포트에 연결한다.
pub(crate) fn production() -> anyhow::Result<Runtime> {
    let usage = UsageApplication::new(vec![
        RegisteredAgent::new(
            AgentInfo {
                name: "claude",
                display: "Claude Code",
            },
            ClaudeUsageSource::default(),
        ),
        RegisteredAgent::new(
            AgentInfo {
                name: "codex",
                display: "Codex",
            },
            CodexUsageSource,
        ),
        RegisteredAgent::new(
            AgentInfo {
                name: "kiro",
                display: "Kiro",
            },
            KiroUsageSource::default(),
        ),
    ])?;

    let repository = FileSettingsRepository;
    let settings_path = repository.path()?;
    let settings = SettingsApplication::new(Arc::new(repository), usage.names());
    let history: Arc<dyn HistoryRepository> = Arc::new(FileHistoryRepository::production());

    Ok(Runtime {
        usage,
        settings,
        settings_path,
        history,
    })
}
