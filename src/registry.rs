//! 이름으로 에이전트를 찾는 표.
//!
//! 새 에이전트를 추가하려면 모듈을 만들고 여기 한 줄만 등록하면 된다.
//! 설정 파일과 `agentmeter` 는 이 표만 보고 동작한다.

use crate::app::Fetch;
use crate::{claude, codex};

pub struct AgentSpec {
    /// 설정 파일에 쓰는 이름
    pub name: &'static str,
    /// 화면에 보이는 이름
    pub display: &'static str,
    /// 이 에이전트만 보는 전용 바이너리
    pub binary: &'static str,
    pub make_fetch: fn(tz: String, live: bool) -> Fetch,
}

static AGENTS: &[AgentSpec] = &[
    AgentSpec {
        name: "claude",
        display: "Claude Code",
        binary: "ccmeter",
        make_fetch: claude_fetch,
    },
    AgentSpec {
        name: "codex",
        display: "Codex",
        binary: "codexmeter",
        make_fetch: codex_fetch,
    },
];

/// 기본은 로컬 캐시를 읽고, 오래됐을 때만 직접 조회한다.
fn claude_fetch(tz: String, live: bool) -> Fetch {
    Box::new(move |force_live| {
        if live || force_live {
            claude::source::fetch_live(&tz)
        } else {
            claude::source::fetch(&tz)
        }
    })
}

/// Codex 는 app-server 호출이 저렴해 캐시 계층이 없다 — `live` 와 무관하다.
fn codex_fetch(tz: String, _live: bool) -> Fetch {
    Box::new(move |_force_live| codex::source::fetch(&tz))
}

pub fn all() -> &'static [AgentSpec] {
    AGENTS
}

pub fn find(name: &str) -> Option<&'static AgentSpec> {
    AGENTS.iter().find(|a| a.name == name)
}

pub fn names() -> Vec<&'static str> {
    AGENTS.iter().map(|a| a.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_is_findable_by_name() {
        for spec in all() {
            assert_eq!(find(spec.name).unwrap().name, spec.name);
        }
    }

    #[test]
    fn unknown_name_is_not_found() {
        assert!(find("gopher").is_none());
    }

    /// 이름이 겹치면 `find` 가 어느 쪽을 줄지 알 수 없다.
    #[test]
    fn names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for spec in all() {
            assert!(seen.insert(spec.name), "이름 중복: {}", spec.name);
        }
    }

    #[test]
    fn names_lists_all_agents() {
        assert_eq!(names().len(), all().len());
    }
}
