//! Codex 한도 조회 진입점.
//!
//! `account/rateLimits/read` 는 호출 제한이 없고 app-server 가 값을 들고 있어
//! 매번 직접 조회해도 무리가 없다. 그래서 Claude provider와 달리 캐시 계층이 없다.

use super::{client, model};
use crate::application::FetchError;
use crate::application::{FetchPolicy, UsageSource};
use crate::domain::usage::UsageSnapshot;

/// Codex app-server를 사용하는 `UsageSource` 어댑터.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodexUsageSource;

impl UsageSource for CodexUsageSource {
    fn fetch(&self, _policy: FetchPolicy) -> Result<UsageSnapshot, FetchError> {
        fetch()
    }
}

pub fn fetch() -> Result<UsageSnapshot, FetchError> {
    client::fetch()
        .map(|response| UsageSnapshot::live(model::to_limits(&response), chrono::Local::now()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::Mutex;

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct FakeCodex {
        root: PathBuf,
        pid_file: PathBuf,
    }

    impl FakeCodex {
        fn install() -> Self {
            let root = std::env::temp_dir().join(format!(
                "agentmeter-fake-codex-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let bin = root.join("codex");
            let pid_file = root.join("pid");
            std::fs::write(
                &bin,
                r##"#!/bin/sh
echo $$ > "$CODEX_TEST_PID"
case "$CODEX_TEST_MODE" in
  success)
    printf '%s\n' '{"id":1,"result":{"codexHome":"/tmp"}}'
    printf '%s\n' '{"method":"remoteControl/status/changed","params":{}}'
    printf '%s\n' '{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":50,"windowDurationMins":10080,"resetsAt":1787196678}}}}'
    ;;
  exit) exit 7 ;;
  timeout) sleep 5 ;;
esac
exec sleep 5
"##,
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&bin).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&bin, permissions).unwrap();
            // SAFETY: ENV_LOCK serializes every mutation in this module.
            unsafe {
                std::env::set_var("CODEX_BIN", &bin);
                std::env::set_var("CODEX_TEST_PID", &pid_file);
            }
            Self { root, pid_file }
        }

        fn pid(&self) -> String {
            std::fs::read_to_string(&self.pid_file)
                .unwrap()
                .trim()
                .to_string()
        }
    }

    impl Drop for FakeCodex {
        fn drop(&mut self) {
            // SAFETY: ENV_LOCK is held for the entire fixture lifetime.
            unsafe {
                std::env::remove_var("CODEX_BIN");
                std::env::remove_var("CODEX_TEST_PID");
                std::env::remove_var("CODEX_TEST_MODE");
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn set_mode(mode: &str) {
        // SAFETY: caller holds ENV_LOCK.
        unsafe { std::env::set_var("CODEX_TEST_MODE", mode) };
    }

    fn process_exists(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn assert_fixture_exists(path: &Path) {
        assert!(path.exists(), "fake Codex should have started");
    }

    #[test]
    fn usage_source_ignores_notifications_and_cleans_up_the_child() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fake = FakeCodex::install();
        set_mode("success");

        let snapshot = CodexUsageSource.fetch(FetchPolicy::Fresh).unwrap();
        assert_eq!(snapshot.limits[0].used_percent, 50.0);
        assert_fixture_exists(&fake.pid_file);
        assert!(!process_exists(&fake.pid()));
    }

    #[test]
    fn usage_source_reports_early_process_exit() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _fake = FakeCodex::install();
        set_mode("exit");

        let error = CodexUsageSource.fetch(FetchPolicy::Fresh).unwrap_err();
        assert!(error.to_string().contains("응답을 마치기 전에 종료"));
    }

    #[test]
    fn usage_source_times_out_and_cleans_up_the_child() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fake = FakeCodex::install();
        set_mode("timeout");

        let error = CodexUsageSource.fetch(FetchPolicy::Fresh).unwrap_err();
        assert!(error.to_string().contains("안에 응답하지 않았습니다"));
        assert!(!process_exists(&fake.pid()));
    }
}
