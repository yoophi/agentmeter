//! 표시할 에이전트를 관리하는 설정 사용 사례.

use std::sync::Arc;

use anyhow::{Result, bail};

use super::SettingsRepository;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// 표시 순서이기도 하므로 입력 순서를 보존한다.
    pub agents: Vec<String>,
}

/// 기본값 선택과 검증을 파일 형식·저장 위치에서 분리한다.
pub(crate) struct SettingsApplication {
    repository: Arc<dyn SettingsRepository>,
    available: Vec<String>,
}

impl SettingsApplication {
    pub(crate) fn new(repository: Arc<dyn SettingsRepository>, available: Vec<String>) -> Self {
        Self {
            repository,
            available,
        }
    }

    /// 저장된 설정을 읽고, 파일이 없으면 등록된 모든 에이전트를 선택한다.
    pub(crate) fn load(&self) -> Result<Settings> {
        let settings = self.repository.load()?.unwrap_or_else(|| Settings {
            agents: self.available.clone(),
        });
        self.validate(&settings)?;
        Ok(settings)
    }

    /// 에이전트 선택을 검증한 뒤 저장한다.
    pub(crate) fn replace_agents(&self, agents: Vec<String>) -> Result<Settings> {
        let settings = Settings { agents };
        self.validate(&settings)?;
        self.repository.save(&settings)?;
        Ok(settings)
    }

    fn validate(&self, settings: &Settings) -> Result<()> {
        if settings.agents.is_empty() {
            bail!("agents 가 비어 있습니다. 예: agents = [\"claude\", \"codex\"]");
        }
        let unknown: Vec<&str> = settings
            .agents
            .iter()
            .map(String::as_str)
            .filter(|name| !self.available.iter().any(|candidate| candidate == name))
            .collect();
        if !unknown.is_empty() {
            bail!(
                "알 수 없는 에이전트: {}. 쓸 수 있는 이름: {}",
                unknown.join(", "),
                self.available.join(", ")
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemorySettings {
        value: Mutex<Option<Settings>>,
    }

    impl SettingsRepository for MemorySettings {
        fn load(&self) -> Result<Option<Settings>> {
            Ok(self.value.lock().unwrap().clone())
        }

        fn save(&self, settings: &Settings) -> Result<()> {
            *self.value.lock().unwrap() = Some(settings.clone());
            Ok(())
        }
    }

    fn names() -> Vec<String> {
        vec!["claude".into(), "codex".into()]
    }

    #[test]
    fn missing_settings_select_every_available_agent() {
        let repository = Arc::new(MemorySettings::default());
        let service = SettingsApplication::new(repository, names());
        assert_eq!(service.load().unwrap().agents, names());
    }

    #[test]
    fn replacement_is_validated_and_persisted() {
        let repository = Arc::new(MemorySettings::default());
        let service = SettingsApplication::new(repository, names());
        let saved = service.replace_agents(vec!["codex".into()]).unwrap();
        assert_eq!(saved.agents, vec!["codex"]);
        assert_eq!(service.load().unwrap(), saved);
        assert!(service.replace_agents(vec!["gopher".into()]).is_err());
    }
}
