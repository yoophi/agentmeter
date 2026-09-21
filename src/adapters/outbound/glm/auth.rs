//! GLM Coding Plan API 키를 찾는다.
//!
//! agentmeter 전용 설정을 새로 만들지 않는다. 이미 GLM 을 쓰고 있다면 키는
//! 환경변수나 opencode 자격증명 중 한 곳에 있다.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

/// 앞에 있는 것이 이긴다. `GLM_TRACKER_ZAI_KEY` 는 glm-tracker 와 같은 키를 쓰기 위함.
const ENV_KEYS: [&str; 2] = ["ZAI_API_KEY", "GLM_TRACKER_ZAI_KEY"];

/// opencode 가 저장하는 provider 이름.
const OPENCODE_PROVIDER: &str = "zai-coding-plan";

pub(crate) fn hint() -> &'static str {
    "no GLM API key found. set ZAI_API_KEY, or sign in to opencode as \
     zai-coding-plan"
}

pub(crate) fn api_key() -> Result<String> {
    for name in ENV_KEYS {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return Ok(value);
            }
        }
    }
    from_opencode()
}

fn opencode_auth_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".local/share/opencode/auth.json"))
}

fn from_opencode() -> Result<String> {
    let Some(path) = opencode_auth_path() else {
        bail!("could not read HOME, so the opencode credentials were not found");
    };
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).context("could not parse the opencode auth.json")?;
    let key = parsed
        .get(OPENCODE_PROVIDER)
        .and_then(|entry| entry.get("key"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty());
    match key {
        Some(key) => Ok(key.to_string()),
        None => bail!("the opencode auth.json has no {OPENCODE_PROVIDER} key"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_opencode_entry_yields_the_key() {
        let raw =
            r#"{"openai":{"type":"oauth"},"zai-coding-plan":{"type":"api","key":" abc123 "}}"#;
        let parsed: serde_json::Value = serde_json::from_str(raw).unwrap();
        let key = parsed
            .get(OPENCODE_PROVIDER)
            .and_then(|entry| entry.get("key"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim);
        assert_eq!(key, Some("abc123"));
    }

    #[test]
    fn the_hint_names_both_sources() {
        assert!(hint().contains("ZAI_API_KEY"));
        assert!(hint().contains(OPENCODE_PROVIDER));
    }
}
