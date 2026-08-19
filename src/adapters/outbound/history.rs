//! 한도 창별 사용률 표본을 JSON 파일로 보존한다.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};

use crate::application::{HistoryRepository, UsageSample};
use crate::domain::usage::{LimitId, UsageWindow};

const FILE_VERSION: u8 = 1;

#[derive(Debug, Clone)]
pub(crate) struct FileHistoryRepository {
    root: Option<PathBuf>,
}

impl FileHistoryRepository {
    pub(crate) fn production() -> Self {
        let root = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".cache/agentmeter/history"));
        Self { root }
    }

    #[cfg(test)]
    fn at(root: PathBuf) -> Self {
        Self { root: Some(root) }
    }

    fn path(&self, provider: &str, window: UsageWindow) -> Option<PathBuf> {
        Some(self.root.as_ref()?.join(file_name(provider, window)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct StoredWindow {
    duration_minutes: i64,
    resets_at_minute: i64,
}

impl StoredWindow {
    fn from(window: UsageWindow) -> Self {
        Self {
            duration_minutes: window.duration.num_minutes(),
            resets_at_minute: (window.resets_at.timestamp() + 30).div_euclid(60),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct HistoryFile {
    version: u8,
    window: StoredWindow,
    series: BTreeMap<String, Vec<UsageSample>>,
}

impl HistoryRepository for FileHistoryRepository {
    fn load(
        &self,
        provider: &str,
        window: UsageWindow,
    ) -> anyhow::Result<BTreeMap<LimitId, Vec<UsageSample>>> {
        let Some(path) = self.path(provider, window) else {
            return Ok(BTreeMap::new());
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("{} 읽기 실패", path.display()));
            }
        };
        let file: HistoryFile =
            serde_json::from_str(&raw).with_context(|| format!("{} 파싱 실패", path.display()))?;
        if file.version != FILE_VERSION || file.window != StoredWindow::from(window) {
            return Ok(BTreeMap::new());
        }
        Ok(file
            .series
            .into_iter()
            .map(|(id, points)| (LimitId::new(id), points))
            .collect())
    }

    fn save(
        &self,
        provider: &str,
        window: UsageWindow,
        series: &BTreeMap<LimitId, Vec<UsageSample>>,
    ) -> anyhow::Result<()> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        std::fs::create_dir_all(root).with_context(|| format!("{} 생성 실패", root.display()))?;
        let path = self.path(provider, window).expect("HOME 이 있는 저장소");
        let file = HistoryFile {
            version: FILE_VERSION,
            window: StoredWindow::from(window),
            series: series
                .iter()
                .map(|(id, points)| (id.as_str().to_string(), points.clone()))
                .collect(),
        };
        let bytes = serde_json::to_vec(&file).context("히스토리 직렬화 실패")?;
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::write(&temporary, bytes)
            .with_context(|| format!("{} 임시 저장 실패", temporary.display()))?;
        std::fs::rename(&temporary, &path)
            .with_context(|| format!("{} 교체 실패", path.display()))?;
        Ok(())
    }
}

fn file_name(provider: &str, window: UsageWindow) -> String {
    let stored = StoredWindow::from(window);
    let duration = match stored.duration_minutes {
        300 => "5H".to_string(),
        10_080 => "7D".to_string(),
        other => format!("{other}M"),
    };
    let start = minute_as_local(stored.resets_at_minute - stored.duration_minutes);
    let end = minute_as_local(stored.resets_at_minute);
    let safe_provider: String = provider
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{safe_provider}__{duration}__{}__{}.json",
        start.format("%Y%m%d%H%M%S"),
        end.format("%Y%m%d%H%M%S")
    )
}

fn minute_as_local(minute: i64) -> chrono::DateTime<Local> {
    Local
        .timestamp_opt(minute * 60, 0)
        .single()
        .expect("정규화한 로컬 시각")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{TimeDelta, TimeZone};

    use super::*;
    use crate::application::{AgentInfo, AgentResult, WatchState};
    use crate::domain::usage::{UsageLimit, UsageSnapshot};

    fn window(hours: i64) -> UsageWindow {
        UsageWindow {
            resets_at: Local
                .with_ymd_and_hms(2026, 8, 20, 12, 0, 0)
                .single()
                .unwrap(),
            duration: TimeDelta::hours(hours),
        }
    }

    #[test]
    fn five_hour_and_seven_day_windows_have_unique_names() {
        assert!(file_name("claude", window(5)).starts_with("claude__5H__"));
        assert!(file_name("codex", window(24 * 7)).starts_with("codex__7D__"));
        assert!(!file_name("claude", window(5)).contains("시작"));
    }

    #[test]
    fn samples_round_trip_for_the_same_window_only() {
        let root = std::env::temp_dir().join(format!(
            "agentmeter-history-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let repository = FileHistoryRepository::at(root.clone());
        let mut series = BTreeMap::new();
        series.insert(
            LimitId::new("session:all"),
            vec![UsageSample {
                minute: 1,
                percent: 42.0,
            }],
        );
        repository.save("claude", window(5), &series).unwrap();
        assert_eq!(repository.load("claude", window(5)).unwrap(), series);

        let later = UsageWindow {
            resets_at: window(5).resets_at + TimeDelta::hours(5),
            ..window(5)
        };
        assert!(repository.load("claude", later).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn restarted_watch_restores_the_current_window() {
        let root = std::env::temp_dir().join(format!(
            "agentmeter-watch-history-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let repository = Arc::new(FileHistoryRepository::at(root.clone()));
        let agent = AgentInfo {
            name: "claude",
            display: "Claude Code",
        };
        let make_snapshot = |percent, minutes| {
            UsageSnapshot::live(
                vec![UsageLimit::new(
                    "session:all",
                    None,
                    percent,
                    None,
                    false,
                    Some(TimeDelta::hours(5)),
                    Some(window(5).resets_at),
                )],
                window(5).started_at() + TimeDelta::minutes(minutes),
            )
        };

        let mut first = WatchState::persistent(vec![agent], repository.clone());
        first.apply(vec![AgentResult {
            agent,
            result: Ok(make_snapshot(20.0, 1)),
        }]);

        let mut restarted = WatchState::persistent(vec![agent], repository);
        restarted.apply(vec![AgentResult {
            agent,
            result: Ok(make_snapshot(30.0, 2)),
        }]);
        let pane = &restarted.panes()[0];
        let active_window = pane.snapshot.as_ref().unwrap().limits[0].window();
        assert_eq!(
            pane.samples(&LimitId::new("session:all"), active_window)
                .len(),
            2
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
