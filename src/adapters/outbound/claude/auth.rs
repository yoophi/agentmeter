//! Claude Code 의 OAuth 자격증명을 **읽기만** 한다.
//!
//! 토큰 갱신은 일부러 하지 않는다. 리프레시 토큰은 회전(rotation)하므로
//! 이 도구가 갱신해서 저장하면 Claude Code 본체의 세션을 무효화할 수 있고,
//! 그 반대도 마찬가지다. 대신 매 폴링마다 자격증명을 다시 읽어서,
//! Claude Code 가 갱신해 둔 새 토큰을 자연스럽게 집어온다.

use anyhow::{Context, Result};
use serde_json::Value;

const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

pub struct Credentials {
    pub access_token: String,
    /// epoch milliseconds
    pub expires_at: Option<i64>,
}

impl Credentials {
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(ms) => chrono::Utc::now().timestamp_millis() >= ms,
            None => false,
        }
    }
}

/// macOS 는 Keychain 을 먼저 보고, 없으면 `~/.claude/.credentials.json`.
/// 그 외 플랫폼은 파일만 본다.
pub fn load() -> Result<Credentials> {
    let raw = read_raw()?;
    parse(&raw)
}

fn read_raw() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        if let Some(s) = read_keychain()? {
            return Ok(s);
        }
    }
    read_file()
}

#[cfg(target_os = "macos")]
fn read_keychain() -> Result<Option<String>> {
    use std::process::Command;

    let out = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .context("could not run the `security` command")?;

    if !out.status.success() {
        // 항목이 없는 경우 — 파일 폴백으로 넘긴다
        return Ok(None);
    }
    let s = String::from_utf8(out.stdout).context("the Keychain value is not UTF-8")?;
    let s = s.trim().to_string();
    if s.is_empty() { Ok(None) } else { Ok(Some(s)) }
}

fn read_file() -> Result<String> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let path = std::path::Path::new(&home).join(".claude/.credentials.json");
    std::fs::read_to_string(&path).with_context(|| {
        format!(
            "no credentials found ({}). check that you are signed in with `claude`",
            path.display()
        )
    })
}

fn parse(raw: &str) -> Result<Credentials> {
    let json: Value = serde_json::from_str(raw).context("could not parse the credential JSON")?;
    let oauth = json
        .get("claudeAiOauth")
        .context("the credentials have no claudeAiOauth")?;

    let access_token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("accessToken is empty")?
        .to_string();

    Ok(Credentials {
        access_token,
        expires_at: oauth.get("expiresAt").and_then(Value::as_i64),
    })
}

/// 만료됐을 때 사용자에게 보여줄 안내. 갱신을 시도하지 않는 이유가 여기 있다.
pub fn reauth_hint() -> &'static str {
    "the access token expired. run `claude` once and it refreshes automatically."
}
