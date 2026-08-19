//! 상주 조회에서 유지해야 하는 상태와 갱신 규칙.

use std::collections::BTreeMap;

use chrono::{DateTime, Local};

use crate::domain::usage::{LimitId, Origin, UsageSnapshot};

use super::{AgentInfo, AgentResult};

const MAX_POINTS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UsageSample {
    pub minute: i64,
    pub percent: f64,
}

pub(crate) struct WatchPane {
    pub agent: AgentInfo,
    pub snapshot: Option<UsageSnapshot>,
    pub error: Option<String>,
    history: BTreeMap<LimitId, Vec<UsageSample>>,
}

impl WatchPane {
    fn new(agent: AgentInfo) -> Self {
        Self {
            agent,
            snapshot: None,
            error: None,
            history: BTreeMap::new(),
        }
    }

    pub(crate) fn samples(&self, id: &LimitId) -> &[UsageSample] {
        self.history.get(id).map(Vec::as_slice).unwrap_or_default()
    }

    fn record(&mut self, snapshot: &UsageSnapshot, at: DateTime<Local>) {
        let minute = at.timestamp() / 60;
        for limit in &snapshot.limits {
            let points = self.history.entry(limit.id.clone()).or_default();
            match points.last_mut() {
                Some(last) if last.minute == minute => last.percent = limit.used_percent,
                _ => points.push(UsageSample {
                    minute,
                    percent: limit.used_percent,
                }),
            }
            if points.len() > MAX_POINTS {
                points.remove(0);
            }
        }
    }
}

pub(crate) struct WatchState {
    panes: Vec<WatchPane>,
}

impl WatchState {
    pub(crate) fn new(agents: Vec<AgentInfo>) -> Self {
        Self {
            panes: agents.into_iter().map(WatchPane::new).collect(),
        }
    }

    pub(crate) fn panes(&self) -> &[WatchPane] {
        &self.panes
    }

    /// 성공은 새 값과 표본을 반영하고, 실패는 직전 값을 보존한 채 오류만 기록한다.
    pub(crate) fn apply(&mut self, results: Vec<AgentResult>, at: DateTime<Local>) {
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
                    pane.record(&snapshot, at);
                    pane.snapshot = Some(snapshot);
                    pane.error = None;
                }
                Err(error) => pane.error = Some(error.to_string()),
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

    fn snapshot(percent: f64) -> UsageSnapshot {
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
            at(0),
        )
    }

    #[test]
    fn failed_refresh_keeps_the_last_snapshot() {
        let mut state = WatchState::new(vec![info()]);
        state.apply(
            vec![AgentResult {
                agent: info(),
                result: Ok(snapshot(42.0)),
            }],
            at(0),
        );
        state.apply(
            vec![AgentResult {
                agent: info(),
                result: Err(FetchError::Other(anyhow::anyhow!("temporary"))),
            }],
            at(1),
        );

        let pane = &state.panes()[0];
        assert_eq!(pane.snapshot.as_ref().unwrap().limits[0].used_percent, 42.0);
        assert_eq!(pane.error.as_deref(), Some("temporary"));
        assert!(state.any_refresh_failed());
    }

    #[test]
    fn samples_are_keyed_by_stable_limit_id() {
        let mut state = WatchState::new(vec![info()]);
        state.apply(
            vec![AgentResult {
                agent: info(),
                result: Ok(snapshot(40.0)),
            }],
            at(0),
        );
        state.apply(
            vec![AgentResult {
                agent: info(),
                result: Ok(snapshot(55.0)),
            }],
            at(1),
        );

        let id = LimitId::new("session:all");
        assert_eq!(state.panes()[0].samples(&id).len(), 2);
    }
}
