//! `agentmeter` 가 어떤 에이전트를 보여줄지 저장한다.
//!
//! 형식은 TOML 이고 위치는 `~/.config/agentmeter/config.toml` 이다
//! (`XDG_CONFIG_HOME` 을 존중한다).
//!
//! ```toml
//! agents = ["claude", "codex"]
//! ```

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const DIR: &str = "agentmeter";
const FILE: &str = "config.toml";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// 표시할 에이전트 이름. 순서가 화면 순서다.
    #[serde(default = "default_agents")]
    pub agents: Vec<String>,
}

fn default_agents() -> Vec<String> {
    crate::registry::all()
        .iter()
        .map(|a| a.name.to_string())
        .collect()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            agents: default_agents(),
        }
    }
}

pub fn path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x),
        _ => PathBuf::from(std::env::var_os("HOME").context("HOME 환경변수가 없습니다")?)
            .join(".config"),
    };
    Ok(base.join(DIR).join(FILE))
}

/// 설정을 읽는다. 파일이 없으면 기본값 — 처음 쓰는 사람이 설정부터 하지 않아도 된다.
pub fn load() -> Result<Config> {
    let p = path()?;
    match std::fs::read_to_string(&p) {
        Ok(raw) => parse(&raw).with_context(|| format!("{} 를 읽을 수 없습니다", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("{} 를 열 수 없습니다", p.display())),
    }
}

pub fn save(cfg: &Config) -> Result<PathBuf> {
    let p = path()?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("{} 를 만들 수 없습니다", dir.display()))?;
    }
    let body = toml::to_string_pretty(cfg).context("설정을 TOML 로 만들 수 없습니다")?;
    std::fs::write(&p, body).with_context(|| format!("{} 에 쓸 수 없습니다", p.display()))?;
    Ok(p)
}

fn parse(raw: &str) -> Result<Config> {
    let cfg: Config = toml::from_str(raw).context("TOML 파싱 실패")?;
    validate(&cfg)?;
    Ok(cfg)
}

/// 모르는 이름이 들어 있으면 조회할 때가 아니라 **읽을 때** 알려준다.
pub fn validate(cfg: &Config) -> Result<()> {
    if cfg.agents.is_empty() {
        bail!("agents 가 비어 있습니다. 예: agents = [\"claude\", \"codex\"]");
    }
    let unknown: Vec<&str> = cfg
        .agents
        .iter()
        .map(String::as_str)
        .filter(|n| crate::registry::find(n).is_none())
        .collect();
    if !unknown.is_empty() {
        bail!(
            "알 수 없는 에이전트: {}. 쓸 수 있는 이름: {}",
            unknown.join(", "),
            crate::registry::names().join(", ")
        );
    }
    Ok(())
}

/// `agents=claude,codex` 를 키와 값으로 나눈다.
pub fn split_assignment(arg: &str) -> Result<(&str, &str)> {
    match arg.split_once('=') {
        Some((k, v)) => Ok((k.trim(), v.trim())),
        None => bail!("`키=값` 형태여야 합니다. 예: agents=claude,codex"),
    }
}

/// 쉼표로 나눈 목록. 빈 항목은 버린다 — `claude,` 같은 오타를 살려 준다.
pub fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_written_config() {
        let cfg = parse("agents = [\"codex\", \"claude\"]").unwrap();
        assert_eq!(cfg.agents, vec!["codex", "claude"]);
    }

    /// 순서가 화면 순서이므로 보존되어야 한다.
    #[test]
    fn keeps_the_written_order() {
        let cfg = parse("agents = [\"codex\", \"claude\"]").unwrap();
        assert_eq!(cfg.agents[0], "codex");
    }

    #[test]
    fn round_trips_through_toml() {
        let cfg = Config {
            agents: vec!["claude".into()],
        };
        let raw = toml::to_string_pretty(&cfg).unwrap();
        assert_eq!(parse(&raw).unwrap(), cfg);
    }

    /// 모르는 이름은 쓸 수 있는 목록과 함께 거절한다.
    #[test]
    fn rejects_unknown_agents() {
        let e = parse("agents = [\"claude\", \"gopher\"]").unwrap_err();
        let msg = format!("{e:#}");
        assert!(msg.contains("gopher"), "{msg}");
        assert!(
            msg.contains("claude"),
            "쓸 수 있는 이름을 알려줘야 함: {msg}"
        );
    }

    #[test]
    fn rejects_empty_agents() {
        assert!(parse("agents = []").is_err());
    }

    /// 필드가 없으면 기본값으로 채운다 — 빈 파일이 오류가 되면 곤란하다.
    #[test]
    fn missing_field_falls_back_to_default() {
        let cfg = parse("").unwrap();
        assert_eq!(cfg.agents, default_agents());
    }

    #[test]
    fn splits_assignments() {
        assert_eq!(
            split_assignment("agents=claude,codex").unwrap(),
            ("agents", "claude,codex")
        );
        assert_eq!(
            split_assignment(" agents = claude ").unwrap(),
            ("agents", "claude")
        );
        assert!(split_assignment("agents").is_err());
    }

    #[test]
    fn splits_lists_and_drops_blanks() {
        assert_eq!(split_list("claude, codex"), vec!["claude", "codex"]);
        assert_eq!(split_list("claude,"), vec!["claude"]);
        assert!(split_list(" , ").is_empty());
    }

    /// XDG_CONFIG_HOME 을 존중한다.
    #[test]
    fn path_follows_xdg() {
        // 환경변수를 건드리지 않고 규칙만 확인한다
        let p = path().unwrap();
        assert!(p.ends_with("agentmeter/config.toml"), "{}", p.display());
    }
}
