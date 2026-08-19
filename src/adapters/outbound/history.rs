//! 한도 창별 사용률 표본을 JSON 파일로 보존한다.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};

use crate::application::{HistoryRepository, HistoryRestore, UsageSample, WindowHistory};
use crate::domain::usage::{LimitId, Origin, UsageLimit, UsageSnapshot, UsageWindow};

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

    fn to_window(self) -> Option<UsageWindow> {
        Some(UsageWindow {
            resets_at: Local
                .timestamp_opt(self.resets_at_minute * 60, 0)
                .single()?,
            duration: chrono::TimeDelta::minutes(self.duration_minutes),
        })
        .filter(|window| window.duration > chrono::TimeDelta::zero())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredLimit {
    scope: Option<String>,
    active: bool,
}

#[derive(Serialize, Deserialize)]
struct HistoryFile {
    version: u8,
    window: StoredWindow,
    series: BTreeMap<String, Vec<UsageSample>>,
    #[serde(default)]
    limits: BTreeMap<String, StoredLimit>,
}

#[derive(Debug)]
struct LoadedHistory {
    window: UsageWindow,
    series: BTreeMap<LimitId, Vec<UsageSample>>,
    limits: BTreeMap<LimitId, StoredLimit>,
}

impl FileHistoryRepository {
    fn load_active_files(
        &self,
        provider: &str,
        at: chrono::DateTime<Local>,
    ) -> anyhow::Result<(Vec<LoadedHistory>, Vec<String>)> {
        let Some(root) = &self.root else {
            return Ok((Vec::new(), Vec::new()));
        };
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Vec::new(), Vec::new()));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("{} 읽기 실패", root.display()));
            }
        };
        let prefix = format!("{}__", safe_provider(provider));
        let mut active = Vec::new();
        let mut warnings = Vec::new();
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    warnings.push(format!("directory entry: {error}"));
                    continue;
                }
            };
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with(&prefix)
                || path.extension().and_then(|ext| ext.to_str()) != Some("json")
            {
                continue;
            }
            let raw = match std::fs::read_to_string(&path) {
                Ok(raw) => raw,
                Err(error) => {
                    warnings.push(format!("{} 읽기 실패: {error}", path.display()));
                    continue;
                }
            };
            let file = match serde_json::from_str::<HistoryFile>(&raw) {
                Ok(file) => file,
                Err(error) => {
                    warnings.push(format!("{} 파싱 실패: {error}", path.display()));
                    continue;
                }
            };
            let Some(window) = file.window.to_window() else {
                warnings.push(format!("{} window 값이 올바르지 않음", path.display()));
                continue;
            };
            if file.version != FILE_VERSION {
                warnings.push(format!(
                    "{} 지원하지 않는 version {}",
                    path.display(),
                    file.version
                ));
                continue;
            }
            if at < window.started_at() || at >= window.resets_at {
                continue;
            }
            active.push(LoadedHistory {
                window,
                series: file
                    .series
                    .into_iter()
                    .map(|(id, points)| (LimitId::new(id), points))
                    .collect(),
                limits: file
                    .limits
                    .into_iter()
                    .map(|(id, limit)| {
                        (
                            LimitId::new(id),
                            StoredLimit {
                                scope: limit.scope,
                                active: limit.active,
                            },
                        )
                    })
                    .collect(),
            });
        }
        Ok((active, warnings))
    }

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
        limits: &[UsageLimit],
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
            limits: limits
                .iter()
                .filter(|limit| belongs_to_window(limit, window))
                .map(|limit| {
                    (
                        limit.id.as_str().to_string(),
                        StoredLimit {
                            scope: limit.scope.clone(),
                            active: limit.active,
                        },
                    )
                })
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

impl HistoryRepository for FileHistoryRepository {
    fn restore_active(
        &self,
        provider: &str,
        at: chrono::DateTime<Local>,
    ) -> anyhow::Result<HistoryRestore> {
        let (mut loaded, warnings) = self.load_active_files(provider, at)?;
        let mut candidates = BTreeMap::<LimitId, RestoredLimit>::new();
        let mut windows = Vec::new();

        for history in &mut loaded {
            merge_legacy_series(&mut history.series, history.window, &mut history.limits);
            let has_metadata = !history.limits.is_empty();
            let has_stable_ids = history.series.keys().any(is_stable_id);
            for (id, points) in &history.series {
                if (has_metadata && !history.limits.contains_key(id))
                    || (!has_metadata && has_stable_ids && !is_stable_id(id))
                {
                    continue;
                }
                let Some(sample) = points.last().copied() else {
                    continue;
                };
                let metadata = history.limits.get(id);
                let candidate = RestoredLimit {
                    sample,
                    window: history.window,
                    scope: metadata
                        .and_then(|limit| limit.scope.clone())
                        .or_else(|| infer_scope(id)),
                    active: metadata.is_some_and(|limit| limit.active),
                };
                if candidates
                    .get(id)
                    .is_none_or(|current| current.sample.minute < sample.minute)
                {
                    candidates.insert(id.clone(), candidate);
                }
            }
            windows.push(WindowHistory {
                window: history.window,
                series: std::mem::take(&mut history.series),
            });
        }

        let snapshot = restored_snapshot(candidates);
        Ok(HistoryRestore {
            snapshot,
            windows,
            warnings,
        })
    }

    fn record(
        &self,
        provider: &str,
        snapshot: &UsageSnapshot,
    ) -> anyhow::Result<Vec<WindowHistory>> {
        let minute = snapshot.origin.at.timestamp().div_euclid(60);
        let mut windows = BTreeMap::<(i64, i64), UsageWindow>::new();
        for limit in &snapshot.limits {
            let Some(window) = limit.window() else {
                continue;
            };
            let stored = StoredWindow::from(window);
            if stored.resets_at_minute - stored.duration_minutes <= minute
                && minute < stored.resets_at_minute
            {
                windows.insert((stored.duration_minutes, stored.resets_at_minute), window);
            }
        }

        let mut recorded = Vec::new();
        for window in windows.into_values() {
            let mut series = self.load(provider, window)?;
            let mut metadata = snapshot
                .limits
                .iter()
                .filter(|limit| belongs_to_window(limit, window))
                .map(|limit| {
                    (
                        limit.id.clone(),
                        StoredLimit {
                            scope: limit.scope.clone(),
                            active: limit.active,
                        },
                    )
                })
                .collect();
            merge_legacy_series(&mut series, window, &mut metadata);
            for limit in snapshot
                .limits
                .iter()
                .filter(|limit| belongs_to_window(limit, window))
            {
                record_sample(
                    series.entry(limit.id.clone()).or_default(),
                    minute,
                    limit.used_percent,
                    window.duration.num_minutes().max(1) as usize,
                );
            }
            self.save(provider, window, &series, &snapshot.limits)?;
            recorded.push(WindowHistory { window, series });
        }
        Ok(recorded)
    }
}

#[derive(Debug)]
struct RestoredLimit {
    sample: UsageSample,
    window: UsageWindow,
    scope: Option<String>,
    active: bool,
}

fn restored_snapshot(candidates: BTreeMap<LimitId, RestoredLimit>) -> Option<UsageSnapshot> {
    let latest_minute = candidates
        .values()
        .map(|candidate| candidate.sample.minute)
        .max()?;
    let captured_at = Local.timestamp_opt(latest_minute * 60, 0).single()?;
    let limits = candidates
        .into_iter()
        .map(|(id, candidate)| {
            UsageLimit::new(
                id.as_str(),
                candidate.scope,
                candidate.sample.percent,
                None,
                candidate.active,
                Some(candidate.window.duration),
                Some(candidate.window.resets_at),
            )
        })
        .collect();
    Some(UsageSnapshot {
        limits,
        origin: Origin::cache(captured_at, false),
    })
}

fn record_sample(points: &mut Vec<UsageSample>, minute: i64, percent: f64, capacity: usize) {
    match points.last_mut() {
        Some(last) if last.minute == minute => last.percent = percent,
        _ => points.push(UsageSample { minute, percent }),
    }
    if points.len() > capacity {
        points.drain(..points.len() - capacity);
    }
}

fn belongs_to_window(limit: &UsageLimit, window: UsageWindow) -> bool {
    limit
        .window()
        .is_some_and(|candidate| StoredWindow::from(candidate) == StoredWindow::from(window))
}

fn is_stable_id(id: &LimitId) -> bool {
    id.as_str().contains(':')
}

fn infer_scope(id: &LimitId) -> Option<String> {
    let value = id.as_str();
    if let Some(scope) = value.strip_prefix("weekly_scoped:") {
        return Some(scope.to_string());
    }
    let (_, scope) = value.rsplit_once('(')?;
    let scope = scope.strip_suffix(')')?;
    (scope != "all models").then(|| scope.to_string())
}

fn merge_legacy_series(
    series: &mut BTreeMap<LimitId, Vec<UsageSample>>,
    window: UsageWindow,
    limits: &mut BTreeMap<LimitId, StoredLimit>,
) {
    let stable_ids: Vec<LimitId> = series
        .keys()
        .filter(|id| is_stable_id(id))
        .cloned()
        .collect();
    let legacy_ids: Vec<LimitId> = series
        .keys()
        .filter(|id| !is_stable_id(id))
        .cloned()
        .collect();
    if legacy_ids.is_empty() {
        return;
    }

    for legacy_id in &legacy_ids {
        let legacy_scope = legacy_scope(legacy_id.as_str(), window);
        let matching: Vec<&LimitId> = stable_ids
            .iter()
            .filter(|id| {
                let stable_scope = limits
                    .get(*id)
                    .and_then(|limit| limit.scope.clone())
                    .or_else(|| infer_scope(id));
                legacy_scope
                    .as_ref()
                    .is_some_and(|scope| *scope == stable_scope)
            })
            .collect();
        let target = match matching.as_slice() {
            [target] => (*target).clone(),
            [] => match stable_id_for_legacy(legacy_id.as_str(), window) {
                Some(target) => target,
                None if stable_ids.len() == 1 && legacy_ids.len() == 1 => stable_ids[0].clone(),
                None => continue,
            },
            _ => continue,
        };
        limits.entry(target.clone()).or_insert_with(|| StoredLimit {
            scope: infer_scope(&target),
            active: false,
        });
        let legacy = series.remove(legacy_id).unwrap_or_default();
        let stable = series.remove(&target).unwrap_or_default();
        let mut by_minute = BTreeMap::new();
        for point in legacy {
            by_minute.insert(point.minute, point.percent);
        }
        for point in stable {
            by_minute.insert(point.minute, point.percent);
        }
        series.insert(
            target,
            by_minute
                .into_iter()
                .map(|(minute, percent)| UsageSample { minute, percent })
                .collect(),
        );
    }
}

fn stable_id_for_legacy(title: &str, window: UsageWindow) -> Option<LimitId> {
    let scope = legacy_scope(title, window)?;
    if window.duration <= chrono::TimeDelta::hours(6) {
        return scope.is_none().then(|| LimitId::new("session:all"));
    }
    if window.duration <= chrono::TimeDelta::days(7) {
        return Some(match scope {
            Some(scope) => LimitId::new(format!("weekly_scoped:{scope}")),
            None => LimitId::new("weekly_all:all"),
        });
    }
    None
}

fn legacy_scope(title: &str, window: UsageWindow) -> Option<Option<String>> {
    let expected_base = if window.duration <= chrono::TimeDelta::hours(6) {
        "Current session"
    } else if window.duration <= chrono::TimeDelta::days(7) {
        "Current week"
    } else {
        return None;
    };
    if title == expected_base {
        return Some(None);
    }
    let scope = title
        .strip_prefix(expected_base)?
        .strip_prefix(" (")?
        .strip_suffix(')')?;
    Some((scope != "all models").then(|| scope.to_string()))
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
    let safe_provider = safe_provider(provider);
    format!(
        "{safe_provider}__{duration}__{}__{}.json",
        start.format("%Y%m%d%H%M%S"),
        end.format("%Y%m%d%H%M%S")
    )
}

fn safe_provider(provider: &str) -> String {
    provider
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
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
        repository.save("claude", window(5), &series, &[]).unwrap();
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

    #[test]
    fn initial_fetch_failure_keeps_active_history_visible() {
        use crate::application::FetchError;

        let root = std::env::temp_dir().join(format!(
            "agentmeter-initial-failure-history-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let repository = Arc::new(FileHistoryRepository::at(root.clone()));
        let agent = AgentInfo {
            name: "claude",
            display: "Claude Code",
        };
        let now = Local::now();
        let session_window = UsageWindow {
            resets_at: now + TimeDelta::hours(1),
            duration: TimeDelta::hours(5),
        };
        let weekly_window = UsageWindow {
            resets_at: now + TimeDelta::days(6),
            duration: TimeDelta::days(7),
        };
        let cached = UsageSnapshot::live(
            vec![
                UsageLimit::new(
                    "session:all",
                    None,
                    42.0,
                    None,
                    false,
                    Some(session_window.duration),
                    Some(session_window.resets_at),
                ),
                UsageLimit::new(
                    "weekly_all:all",
                    None,
                    23.0,
                    None,
                    true,
                    Some(weekly_window.duration),
                    Some(weekly_window.resets_at),
                ),
                UsageLimit::new(
                    "weekly_scoped:Fable",
                    Some("Fable".to_string()),
                    19.0,
                    None,
                    false,
                    Some(weekly_window.duration),
                    Some(weekly_window.resets_at),
                ),
            ],
            now - TimeDelta::minutes(1),
        );
        let mut first = WatchState::persistent(vec![agent], repository.clone());
        first.apply(vec![AgentResult {
            agent,
            result: Ok(cached),
        }]);

        let mut restarted = WatchState::persistent(vec![agent], repository);
        restarted.apply(vec![AgentResult {
            agent,
            result: Err(FetchError::Other(anyhow::anyhow!("HTTP 429"))),
        }]);

        let pane = &restarted.panes()[0];
        let limits = &pane.snapshot.as_ref().unwrap().limits;
        assert_eq!(limits.len(), 3);
        assert!(limits.iter().any(|limit| {
            limit.window_duration == Some(TimeDelta::hours(5)) && limit.used_percent == 42.0
        }));
        assert_eq!(
            limits
                .iter()
                .find(|limit| limit.scope.as_deref() == Some("Fable"))
                .unwrap()
                .used_percent,
            19.0
        );
        assert!(pane.snapshot.as_ref().unwrap().origin.refresh_failed);
        assert_eq!(pane.error.as_deref(), Some("HTTP 429"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_title_samples_are_restored_under_the_stable_id() {
        let root = std::env::temp_dir().join(format!(
            "agentmeter-legacy-history-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let repository = Arc::new(FileHistoryRepository::at(root.clone()));
        let now = Local::now();
        let active_window = UsageWindow {
            resets_at: now + TimeDelta::hours(1),
            duration: TimeDelta::hours(5),
        };
        let minute = now.timestamp().div_euclid(60);
        let mut series = BTreeMap::new();
        series.insert(
            LimitId::new("Current session"),
            vec![
                UsageSample {
                    minute: minute - 3,
                    percent: 10.0,
                },
                UsageSample {
                    minute: minute - 2,
                    percent: 20.0,
                },
                UsageSample {
                    minute: minute - 1,
                    percent: 30.0,
                },
            ],
        );
        series.insert(
            LimitId::new("session:all"),
            vec![UsageSample {
                minute,
                percent: 40.0,
            }],
        );
        repository
            .save("claude", active_window, &series, &[])
            .unwrap();

        let weekly_window = UsageWindow {
            resets_at: now + TimeDelta::days(6),
            duration: TimeDelta::days(7),
        };
        let mut weekly_series = BTreeMap::new();
        for (legacy, stable, base) in [
            ("Current week (all models)", "weekly_all:all", 20.0),
            ("Current week (Fable)", "weekly_scoped:Fable", 30.0),
        ] {
            weekly_series.insert(
                LimitId::new(legacy),
                vec![
                    UsageSample {
                        minute: minute - 2,
                        percent: base,
                    },
                    UsageSample {
                        minute: minute - 1,
                        percent: base + 1.0,
                    },
                ],
            );
            weekly_series.insert(
                LimitId::new(stable),
                vec![UsageSample {
                    minute,
                    percent: base + 2.0,
                }],
            );
        }
        repository
            .save("claude", weekly_window, &weekly_series, &[])
            .unwrap();

        let state = WatchState::persistent(
            vec![AgentInfo {
                name: "claude",
                display: "Claude Code",
            }],
            repository,
        );
        let pane = &state.panes()[0];
        let restored_window = pane.snapshot.as_ref().unwrap().limits[0].window();
        let points = pane.samples(&LimitId::new("session:all"), restored_window);
        assert_eq!(points.len(), 4);
        assert_eq!(points.last().unwrap().percent, 40.0);
        for id in ["weekly_all:all", "weekly_scoped:Fable"] {
            let limit = pane
                .snapshot
                .as_ref()
                .unwrap()
                .limits
                .iter()
                .find(|limit| limit.id == LimitId::new(id))
                .unwrap();
            assert_eq!(pane.samples(&limit.id, limit.window()).len(), 3);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn partial_restore_keeps_valid_windows_and_reports_corrupt_files() {
        let root = std::env::temp_dir().join(format!(
            "agentmeter-partial-history-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let repository = Arc::new(FileHistoryRepository::at(root.clone()));
        let now = Local::now();
        let active = UsageWindow {
            resets_at: now + TimeDelta::hours(1),
            duration: TimeDelta::hours(5),
        };
        let snapshot = UsageSnapshot::live(
            vec![UsageLimit::new(
                "session:all",
                None,
                42.0,
                None,
                false,
                Some(active.duration),
                Some(active.resets_at),
            )],
            now,
        );
        repository.record("claude", &snapshot).unwrap();
        std::fs::write(root.join("claude__5H__broken__window.json"), "not json").unwrap();

        let state = WatchState::persistent(
            vec![AgentInfo {
                name: "claude",
                display: "Claude Code",
            }],
            repository,
        );
        let pane = &state.panes()[0];
        assert_eq!(pane.snapshot.as_ref().unwrap().limits[0].used_percent, 42.0);
        assert!(
            pane.error
                .as_deref()
                .unwrap()
                .contains("히스토리 부분 복원")
        );
        assert!(pane.error.as_deref().unwrap().contains("파싱 실패"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn same_minute_weekly_resets_restore_every_limit() {
        let root = std::env::temp_dir().join(format!(
            "agentmeter-weekly-reset-history-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let repository = FileHistoryRepository::at(root.clone());
        let now = Local::now();
        let first_reset = now + TimeDelta::days(6);
        let second_reset = first_reset + TimeDelta::microseconds(250);
        let snapshot = UsageSnapshot::live(
            vec![
                UsageLimit::new(
                    "weekly_all:all",
                    None,
                    23.0,
                    None,
                    true,
                    Some(TimeDelta::days(7)),
                    Some(first_reset),
                ),
                UsageLimit::new(
                    "weekly_scoped:Fable",
                    Some("Fable".into()),
                    19.0,
                    None,
                    false,
                    Some(TimeDelta::days(7)),
                    Some(second_reset),
                ),
            ],
            now,
        );

        repository.record("claude", &snapshot).unwrap();
        let restored = repository.restore_active("claude", now).unwrap();
        let ids: Vec<_> = restored
            .snapshot
            .unwrap()
            .limits
            .into_iter()
            .map(|limit| limit.id.as_str().to_string())
            .collect();
        assert_eq!(ids, vec!["weekly_all:all", "weekly_scoped:Fable"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn incomplete_metadata_restores_legacy_weekly_all_series() {
        let root = std::env::temp_dir().join(format!(
            "agentmeter-incomplete-weekly-history-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let repository = FileHistoryRepository::at(root.clone());
        let now = Local::now();
        let weekly = UsageWindow {
            resets_at: now + TimeDelta::days(6),
            duration: TimeDelta::days(7),
        };
        let minute = now.timestamp().div_euclid(60);
        let series = BTreeMap::from([
            (
                LimitId::new("Current week (all models)"),
                vec![UsageSample {
                    minute,
                    percent: 23.0,
                }],
            ),
            (
                LimitId::new("weekly_scoped:Fable"),
                vec![UsageSample {
                    minute,
                    percent: 19.0,
                }],
            ),
        ]);
        let fable_only = vec![UsageLimit::new(
            "weekly_scoped:Fable",
            Some("Fable".into()),
            19.0,
            None,
            false,
            Some(weekly.duration),
            Some(weekly.resets_at),
        )];
        repository
            .save("claude", weekly, &series, &fable_only)
            .unwrap();

        let restored = repository.restore_active("claude", now).unwrap();
        let limits = restored.snapshot.unwrap().limits;
        assert!(limits.iter().any(|limit| {
            limit.id == LimitId::new("weekly_all:all") && limit.used_percent == 23.0
        }));
        let _ = std::fs::remove_dir_all(root);
    }
}
