//! 공식 Kiro CLI를 제한 시간 안에서 실행한다.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};

use crate::application::FetchError;

#[cfg(not(test))]
const TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(test)]
const TIMEOUT: Duration = Duration::from_secs(1);

fn kiro_bin() -> String {
    std::env::var("KIRO_BIN").unwrap_or_else(|_| "kiro-cli".to_string())
}

pub(super) fn fetch() -> Result<String, FetchError> {
    let bin = kiro_bin();
    let mut child = Command::new(&bin)
        .args(["chat", "--no-interactive", "/usage"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!("`{bin}`를 실행할 수 없습니다. Kiro CLI가 설치되어 있는지 확인하세요")
        })
        .map_err(FetchError::Other)?;

    let stdout = child
        .stdout
        .take()
        .context("Kiro CLI stdout을 열 수 없습니다")
        .map_err(FetchError::Other)?;
    let stderr = child
        .stderr
        .take()
        .context("Kiro CLI stderr를 열 수 없습니다")
        .map_err(FetchError::Other)?;
    let stdout = thread::spawn(move || read(stdout));
    let stderr = thread::spawn(move || read(stderr));
    let deadline = Instant::now() + TIMEOUT;

    let status = loop {
        match child
            .try_wait()
            .map_err(|error| FetchError::Other(error.into()))?
        {
            Some(status) => break status,
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout.join();
                let _ = stderr.join();
                return Err(FetchError::Other(anyhow!(
                    "Kiro CLI가 {}초 안에 응답하지 않았습니다",
                    TIMEOUT.as_secs()
                )));
            }
        }
    };
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    let combined = format!("{out}\n{err}");
    if status.success() {
        return Ok(combined);
    }
    let lower = combined.to_lowercase();
    if lower.contains("login")
        || lower.contains("logged in")
        || lower.contains("unauthorized")
        || lower.contains("authenticate")
    {
        return Err(FetchError::Unauthorized(format!(
            "Kiro CLI 로그인이 필요합니다 — `kiro-cli login`으로 로그인하세요: {}",
            combined.trim()
        )));
    }
    Err(FetchError::Other(anyhow!(
        "Kiro CLI가 종료 코드 {}를 반환했습니다: {}",
        status
            .code()
            .map_or_else(|| "unknown".into(), |code| code.to_string()),
        combined.trim()
    )))
}

fn read(mut pipe: impl Read) -> String {
    let mut bytes = Vec::new();
    let _ = pipe.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct FakeKiro {
        root: PathBuf,
    }

    impl FakeKiro {
        fn install(script: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "agentmeter-fake-kiro-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let bin = root.join("kiro-cli");
            std::fs::write(&bin, script).unwrap();
            let mut permissions = std::fs::metadata(&bin).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&bin, permissions).unwrap();
            // SAFETY: ENV_LOCK serializes every KIRO_BIN mutation in this module.
            unsafe { std::env::set_var("KIRO_BIN", &bin) };
            Self { root }
        }
    }

    impl Drop for FakeKiro {
        fn drop(&mut self) {
            // SAFETY: the fixture lives while ENV_LOCK is held.
            unsafe { std::env::remove_var("KIRO_BIN") };
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn invokes_the_official_non_interactive_usage_command() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _fake = FakeKiro::install(
            "#!/bin/sh\n\
             test \"$1\" = chat || exit 9\n\
             test \"$2\" = --no-interactive || exit 9\n\
             test \"$3\" = /usage || exit 9\n\
             printf '%s\\n' 'Estimated Usage | resets on 2026-09-01 | KIRO POWER'\n",
        );
        let output = fetch().unwrap();
        assert!(output.contains("Estimated Usage"));
    }

    #[test]
    fn turns_login_failures_into_reauthentication_guidance() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _fake = FakeKiro::install("#!/bin/sh\necho 'not logged in' >&2\nexit 1\n");
        let error = fetch().unwrap_err();
        assert!(matches!(error, FetchError::Unauthorized(_)));
        assert!(error.to_string().contains("kiro-cli login"));
    }
}
