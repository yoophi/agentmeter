//! 공급자 선택과 병렬 조회를 캡슐화한 애플리케이션 사용 사례.

use std::collections::HashSet;
use std::sync::Arc;
use std::thread;

use anyhow::{Result, bail};

use crate::domain::usage::UsageSnapshot;

use super::{FetchError, FetchPolicy, UsageSource};

/// 출력 어댑터에 전달해도 되는 불변 메타데이터.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentInfo {
    pub name: &'static str,
    pub display: &'static str,
}

/// composition root만 만드는 공급자 등록 정보.
#[derive(Clone)]
pub(crate) struct RegisteredAgent {
    info: AgentInfo,
    source: Arc<dyn UsageSource>,
}

impl RegisteredAgent {
    pub(crate) fn new(info: AgentInfo, source: impl UsageSource + 'static) -> Self {
        Self {
            info,
            source: Arc::new(source),
        }
    }
}

/// 공급자 하나의 조회 결과. 실패해도 다른 공급자 결과는 유지한다.
#[derive(Debug)]
pub(crate) struct AgentResult {
    pub agent: AgentInfo,
    pub result: Result<UsageSnapshot, FetchError>,
}

#[derive(Clone)]
pub(crate) struct UsageApplication {
    agents: Vec<RegisteredAgent>,
}

impl UsageApplication {
    pub(crate) fn new(agents: Vec<RegisteredAgent>) -> Result<Self> {
        if agents.is_empty() {
            bail!("사용량 공급자가 하나 이상 필요합니다");
        }
        let mut names = HashSet::new();
        for agent in &agents {
            if !names.insert(agent.info.name) {
                bail!("에이전트 이름이 중복되었습니다: {}", agent.info.name);
            }
        }
        Ok(Self { agents })
    }

    pub(crate) fn names(&self) -> Vec<String> {
        self.agents
            .iter()
            .map(|agent| agent.info.name.to_string())
            .collect()
    }

    pub(crate) fn info(&self, names: &[String]) -> Result<Vec<AgentInfo>> {
        names
            .iter()
            .map(|name| self.registered(name).map(|agent| agent.info))
            .collect()
    }

    /// 이름 해석과 조회를 한 경계 안에서 수행하며 요청 순서를 보존한다.
    pub(crate) fn query(&self, names: &[String], policy: FetchPolicy) -> Result<Vec<AgentResult>> {
        let selected: Vec<RegisteredAgent> = names
            .iter()
            .map(|name| self.registered(name).cloned())
            .collect::<Result<_>>()?;

        Ok(thread::scope(|scope| {
            let handles: Vec<_> = selected
                .into_iter()
                .map(|agent| {
                    let info = agent.info;
                    let handle = scope.spawn(move || agent.source.fetch(policy));
                    (info, handle)
                })
                .collect();

            handles
                .into_iter()
                .map(|(agent, handle)| AgentResult {
                    agent,
                    result: handle.join().unwrap_or_else(|_| {
                        Err(FetchError::Other(anyhow::anyhow!(
                            "조회 중 오류가 발생했습니다"
                        )))
                    }),
                })
                .collect()
        }))
    }

    fn registered(&self, name: &str) -> Result<&RegisteredAgent> {
        self.agents
            .iter()
            .find(|agent| agent.info.name == name)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "알 수 없는 에이전트: {name}. 쓸 수 있는 이름: {}",
                    self.names().join(", ")
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubSource(Result<UsageSnapshot, &'static str>);

    impl UsageSource for StubSource {
        fn fetch(&self, _policy: FetchPolicy) -> Result<UsageSnapshot, FetchError> {
            match &self.0 {
                Ok(snapshot) => Ok(snapshot.clone()),
                Err(message) => Err(FetchError::Other(anyhow::anyhow!(*message))),
            }
        }
    }

    fn agent(name: &'static str, result: Result<UsageSnapshot, &'static str>) -> RegisteredAgent {
        RegisteredAgent::new(
            AgentInfo {
                name,
                display: name,
            },
            StubSource(result),
        )
    }

    fn snapshot() -> UsageSnapshot {
        UsageSnapshot::live(vec![], chrono::Local::now())
    }

    #[test]
    fn query_keeps_the_requested_order() {
        let application = UsageApplication::new(vec![
            agent("claude", Ok(snapshot())),
            agent("codex", Ok(snapshot())),
        ])
        .unwrap();
        let panes = application
            .query(
                &["codex".into(), "claude".into()],
                FetchPolicy::PreferCached,
            )
            .unwrap();
        assert_eq!(
            panes.iter().map(|pane| pane.agent.name).collect::<Vec<_>>(),
            vec!["codex", "claude"]
        );
    }

    #[test]
    fn one_failure_does_not_hide_other_results() {
        let application = UsageApplication::new(vec![
            agent("good", Ok(snapshot())),
            agent("bad", Err("조회 실패")),
        ])
        .unwrap();
        let panes = application
            .query(&["good".into(), "bad".into()], FetchPolicy::PreferCached)
            .unwrap();
        assert!(panes[0].result.is_ok());
        assert_eq!(
            panes[1].result.as_ref().unwrap_err().to_string(),
            "조회 실패"
        );
    }

    #[test]
    fn unknown_names_are_rejected_at_the_use_case_boundary() {
        let application = UsageApplication::new(vec![agent("claude", Ok(snapshot()))]).unwrap();
        let result = application.query(&["gopher".into()], FetchPolicy::PreferCached);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("알 수 없는 에이전트")
        );
    }

    #[test]
    fn duplicate_agent_names_are_rejected() {
        let result = UsageApplication::new(vec![
            agent("same", Ok(snapshot())),
            agent("same", Ok(snapshot())),
        ]);
        assert!(result.is_err());
    }
}
