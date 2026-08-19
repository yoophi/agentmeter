//! 여러 에이전트를 한 번에 조회한다.
//!
//! 조회는 스레드로 **동시에** 한다. 순차로 하면 가장 느린 에이전트가
//! 전체 대기 시간을 결정한다 (Codex 는 app-server 를 띄우느라 ~1초 걸린다).

use std::thread;

use crate::FetchError;
use crate::meter::Snapshot;
use crate::registry::{self, AgentSpec};

/// 에이전트 하나의 조회 결과. 실패해도 다른 에이전트는 계속 보여준다.
pub struct Pane {
    pub agent: &'static AgentSpec,
    pub result: Result<Snapshot, FetchError>,
}

/// 설정에 적힌 순서대로 조회한다. 순서가 화면 순서다.
pub fn fetch_all(agents: &[String], tz: &str, live: bool) -> Vec<Pane> {
    let specs: Vec<&'static AgentSpec> = agents.iter().filter_map(|n| registry::find(n)).collect();

    thread::scope(|scope| {
        let handles: Vec<_> = specs
            .iter()
            .map(|spec| {
                let tz = tz.to_string();
                let make = spec.make_fetch;
                scope.spawn(move || (make)(tz, live)())
            })
            .collect();

        specs
            .into_iter()
            .zip(handles)
            .map(|(agent, h)| Pane {
                agent,
                result: h.join().unwrap_or_else(|_| {
                    Err(FetchError::Other(anyhow::anyhow!(
                        "조회 중 오류가 발생했습니다"
                    )))
                }),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 모르는 이름은 조용히 건너뛴다 — 검증은 설정을 읽을 때 이미 했다.
    #[test]
    fn unknown_agents_are_skipped() {
        let panes = fetch_all(&["gopher".to_string()], "Asia/Seoul", false);
        assert!(panes.is_empty());
    }

    /// 설정 순서가 화면 순서다.
    #[test]
    fn keeps_the_requested_order() {
        let panes = fetch_all(
            &["codex".to_string(), "claude".to_string()],
            "Asia/Seoul",
            false,
        );
        let names: Vec<&str> = panes.iter().map(|p| p.agent.name).collect();
        assert_eq!(names, vec!["codex", "claude"]);
    }
}
