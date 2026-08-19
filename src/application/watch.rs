//! 상주 조회에서 유지해야 하는 상태와 갱신 규칙.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::domain::usage::{LimitId, Origin, UsageSnapshot, UsageWindow};

use super::{AgentInfo, AgentResult, HistoryRepository, HistoryRestore, WindowHistory};

const MAX_POINTS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct UsageSample {
    pub minute: i64,
    pub percent: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct WindowKey {
    duration_minutes: i64,
    resets_at_minute: i64,
}

impl WindowKey {
    fn from(window: UsageWindow) -> Self {
        Self {
            duration_minutes: window.duration.num_minutes(),
            resets_at_minute: (window.resets_at.timestamp() + 30).div_euclid(60),
        }
    }

    fn contains(self, minute: i64) -> bool {
        self.resets_at_minute - self.duration_minutes <= minute && minute < self.resets_at_minute
    }
}

pub(crate) struct WatchPane {
    pub agent: AgentInfo,
    pub snapshot: Option<UsageSnapshot>,
    pub error: Option<String>,
    memory_history: BTreeMap<LimitId, Vec<UsageSample>>,
    window_history: BTreeMap<WindowKey, BTreeMap<LimitId, Vec<UsageSample>>>,
    history_warning: Option<String>,
    repository: Option<Arc<dyn HistoryRepository>>,
}

impl WatchPane {
    fn new(agent: AgentInfo, repository: Option<Arc<dyn HistoryRepository>>) -> Self {
        let mut pane = Self {
            agent,
            snapshot: None,
            error: None,
            memory_history: BTreeMap::new(),
            window_history: BTreeMap::new(),
            history_warning: None,
            repository,
        };
        if let Some(repository) = &pane.repository {
            match repository.restore_active(pane.agent.name, Local::now()) {
                Ok(history) => pane.restore(history),
                Err(error) => pane.error = Some(format!("히스토리 복원 실패: {error:#}")),
            }
        }
        pane
    }

    fn restore(&mut self, restore: HistoryRestore) {
        self.snapshot = restore.snapshot;
        self.replace_windows(restore.windows);
        if !restore.warnings.is_empty() {
            self.history_warning = Some(format!(
                "히스토리 부분 복원: {}",
                restore.warnings.join(" · ")
            ));
            self.error = self.history_warning.clone();
        }
    }

    pub(crate) fn samples(&self, id: &LimitId, window: Option<UsageWindow>) -> &[UsageSample] {
        match window {
            Some(window) => self
                .window_history
                .get(&WindowKey::from(window))
                .and_then(|series| series.get(id))
                .map(Vec::as_slice)
                .unwrap_or_default(),
            None => self
                .memory_history
                .get(id)
                .map(Vec::as_slice)
                .unwrap_or_default(),
        }
    }

    fn record(&mut self, snapshot: &UsageSnapshot) -> anyhow::Result<()> {
        let minute = snapshot.origin.at.timestamp() / 60;
        for limit in &snapshot.limits {
            if limit.window().is_none() {
                let points = self.memory_history.entry(limit.id.clone()).or_default();
                record_sample(points, minute, limit.used_percent, MAX_POINTS);
            }
        }

        if let Some(repository) = &self.repository {
            let windows = repository.record(self.agent.name, snapshot)?;
            self.replace_windows(windows);
        } else {
            for limit in &snapshot.limits {
                let Some(window) = limit.window() else {
                    continue;
                };
                let key = WindowKey::from(window);
                if !key.contains(minute) {
                    continue;
                }
                let points = self
                    .window_history
                    .entry(key)
                    .or_default()
                    .entry(limit.id.clone())
                    .or_default();
                record_sample(
                    points,
                    minute,
                    limit.used_percent,
                    key.duration_minutes as usize,
                );
            }
        }
        Ok(())
    }

    fn replace_windows(&mut self, windows: Vec<WindowHistory>) {
        for history in windows {
            self.window_history
                .insert(WindowKey::from(history.window), history.series);
        }
    }
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

pub(crate) struct WatchState {
    panes: Vec<WatchPane>,
}

impl WatchState {
    #[cfg(test)]
    pub(crate) fn new(agents: Vec<AgentInfo>) -> Self {
        Self::with_repository(agents, None)
    }

    pub(crate) fn persistent(
        agents: Vec<AgentInfo>,
        repository: Arc<dyn HistoryRepository>,
    ) -> Self {
        Self::with_repository(agents, Some(repository))
    }

    fn with_repository(
        agents: Vec<AgentInfo>,
        repository: Option<Arc<dyn HistoryRepository>>,
    ) -> Self {
        Self {
            panes: agents
                .into_iter()
                .map(|agent| WatchPane::new(agent, repository.clone()))
                .collect(),
        }
    }

    pub(crate) fn panes(&self) -> &[WatchPane] {
        &self.panes
    }

    /// 성공은 새 값과 실측 시각의 표본을 반영하고, 실패는 직전 값을 보존한다.
    pub(crate) fn apply(&mut self, results: Vec<AgentResult>) {
        for result in results {
            let Some(pane) = self
                .panes
                .iter_mut()
                .find(|pane| pane.agent.name == result.agent.name)
            else {
                continue;
            };
            match result.result {
                Ok(snapshot) => {
                    let history_error = pane.record(&snapshot).err();
                    pane.snapshot = Some(snapshot);
                    pane.error = history_error
                        .map(|error| format!("히스토리 저장 실패: {error:#}"))
                        .or_else(|| pane.history_warning.clone());
                }
                Err(error) => {
                    if let Some(snapshot) = &mut pane.snapshot {
                        snapshot.origin.refresh_failed = true;
                    }
                    pane.error = Some(match &pane.history_warning {
                        Some(warning) => format!("{error} · {warning}"),
                        None => error.to_string(),
                    });
                }
            }
        }
    }

    pub(crate) fn oldest_origin(&self, now: DateTime<Local>) -> Option<Origin> {
        self.panes
            .iter()
            .filter_map(|pane| pane.snapshot.as_ref().map(|snapshot| snapshot.origin))
            .max_by_key(|origin| origin.age_seconds(now))
    }

    pub(crate) fn any_refresh_failed(&self) -> bool {
        self.panes
            .iter()
            .any(|pane| pane.error.is_some() && pane.snapshot.is_some())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};

    use super::*;
    use crate::application::FetchError;
    use crate::domain::usage::UsageLimit;

    fn at(minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 8, 19, 12, minute, 0)
            .single()
            .unwrap()
    }

    fn info() -> AgentInfo {
        AgentInfo {
            name: "claude",
            display: "Claude Code",
        }
    }

    fn snapshot(percent: f64, minute: u32) -> UsageSnapshot {
        UsageSnapshot::live(
            vec![UsageLimit::new(
                "session:all",
                None,
                percent,
                None,
                false,
                Some(TimeDelta::hours(5)),
                Some(at(30)),
            )],
            at(minute),
        )
    }

    #[test]
    fn failed_refresh_keeps_the_last_snapshot() {
        let mut state = WatchState::new(vec![info()]);
        state.apply(vec![AgentResult {
            agent: info(),
            result: Ok(snapshot(42.0, 0)),
        }]);
        state.apply(vec![AgentResult {
            agent: info(),
            result: Err(FetchError::Other(anyhow::anyhow!("temporary"))),
        }]);
        let pane = &state.panes()[0];
        assert_eq!(pane.snapshot.as_ref().unwrap().limits[0].used_percent, 42.0);
        assert_eq!(pane.error.as_deref(), Some("temporary"));
        assert!(state.any_refresh_failed());
    }

    #[test]
    fn samples_use_snapshot_origin_and_stable_limit_id() {
        let mut state = WatchState::new(vec![info()]);
        state.apply(vec![AgentResult {
            agent: info(),
            result: Ok(snapshot(40.0, 0)),
        }]);
        state.apply(vec![AgentResult {
            agent: info(),
            result: Ok(snapshot(55.0, 1)),
        }]);
        let pane = &state.panes()[0];
        let window = pane.snapshot.as_ref().unwrap().limits[0].window();
        let points = pane.samples(&LimitId::new("session:all"), window);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].minute, at(0).timestamp() / 60);
    }
}
