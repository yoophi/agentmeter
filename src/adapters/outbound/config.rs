//! TOML 파일로 설정 포트를 구현한다.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::application::{Settings, SettingsRepository};

const DIR: &str = "agentmeter";
const FILE: &str = "config.toml";

#[derive(Debug, Default, Clone, Copy)]
pub struct FileSettingsRepository;

impl FileSettingsRepository {
    pub(crate) fn path(&self) -> Result<PathBuf> {
        let base = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(xdg) if !xdg.is_empty() => PathBuf::from(xdg),
            _ => PathBuf::from(std::env::var_os("HOME").context("HOME 환경변수가 없습니다")?)
                .join(".config"),
        };
        Ok(base.join(DIR).join(FILE))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSettings {
    agents: Option<Vec<String>>,
}

impl SettingsRepository for FileSettingsRepository {
    fn load(&self) -> Result<Option<Settings>> {
        let path = self.path()?;
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("{} 를 열 수 없습니다", path.display()));
            }
        };
        parse(&raw).with_context(|| format!("{} 를 읽을 수 없습니다", path.display()))
    }

    fn save(&self, settings: &Settings) -> Result<()> {
        let path = self.path()?;
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)
                .with_context(|| format!("{} 를 만들 수 없습니다", directory.display()))?;
        }
        let stored = StoredSettings {
            agents: Some(settings.agents.clone()),
        };
        let body = toml::to_string_pretty(&stored).context("설정을 TOML 로 만들 수 없습니다")?;
        std::fs::write(&path, body)
            .with_context(|| format!("{} 에 쓸 수 없습니다", path.display()))?;
        Ok(())
    }
}

fn parse(raw: &str) -> Result<Option<Settings>> {
    let stored: StoredSettings = toml::from_str(raw).context("TOML 파싱 실패")?;
    Ok(stored.agents.map(|agents| Settings { agents }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_written_config_and_keeps_order() {
        let settings = parse("agents = [\"codex\", \"claude\"]").unwrap().unwrap();
        assert_eq!(settings.agents, vec!["codex", "claude"]);
    }

    #[test]
    fn missing_field_delegates_defaulting_to_the_application() {
        assert!(parse("").unwrap().is_none());
    }

    #[test]
    fn path_follows_xdg_shape() {
        let path = FileSettingsRepository.path().unwrap();
        assert!(
            path.ends_with("agentmeter/config.toml"),
            "{}",
            path.display()
        );
    }
}
