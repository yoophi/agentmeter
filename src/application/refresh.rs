//! TUI와 웹이 공유하는 연속 조회의 의미 규칙.

use std::sync::Mutex;

use super::FetchPolicy;

/// 실행 중인 조회와 하나의 병합된 대기 요청을 관리한다.
pub(crate) struct RefreshCoordinator {
    always_fresh: bool,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    running: bool,
    pending: Option<FetchPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshDecision {
    Execute(FetchPolicy),
    Queued,
}

impl RefreshCoordinator {
    pub(crate) fn new(always_fresh: bool) -> Self {
        Self {
            always_fresh,
            state: Mutex::new(State::default()),
        }
    }

    /// idle이면 바로 실행할 policy를 돌려주고, 실행 중이면 요청을 하나로 병합한다.
    pub(crate) fn request(&self, force_fresh: bool) -> RefreshDecision {
        let requested = self.policy(force_fresh);
        let mut state = self.state.lock().expect("refresh state lock");
        if !state.running {
            state.running = true;
            return RefreshDecision::Execute(requested);
        }
        state.pending = Some(stronger(state.pending, requested));
        RefreshDecision::Queued
    }

    /// 현재 조회를 끝내고, 병합된 요청이 있으면 다음 policy를 돌려준다.
    pub(crate) fn complete(&self) -> Option<FetchPolicy> {
        let mut state = self.state.lock().expect("refresh state lock");
        match state.pending.take() {
            Some(policy) => Some(policy),
            None => {
                state.running = false;
                None
            }
        }
    }

    fn policy(&self, force_fresh: bool) -> FetchPolicy {
        if self.always_fresh || force_fresh {
            FetchPolicy::Fresh
        } else {
            FetchPolicy::PreferCached
        }
    }
}

fn stronger(current: Option<FetchPolicy>, requested: FetchPolicy) -> FetchPolicy {
    if current == Some(FetchPolicy::Fresh) || requested == FetchPolicy::Fresh {
        FetchPolicy::Fresh
    } else {
        FetchPolicy::PreferCached
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_request_starts_immediately() {
        let refresh = RefreshCoordinator::new(false);
        assert_eq!(
            refresh.request(false),
            RefreshDecision::Execute(FetchPolicy::PreferCached)
        );
        assert_eq!(refresh.complete(), None);
    }

    #[test]
    fn pending_requests_coalesce_and_fresh_wins() {
        let refresh = RefreshCoordinator::new(false);
        assert!(matches!(
            refresh.request(false),
            RefreshDecision::Execute(_)
        ));
        assert_eq!(refresh.request(false), RefreshDecision::Queued);
        assert_eq!(refresh.request(true), RefreshDecision::Queued);
        assert_eq!(refresh.complete(), Some(FetchPolicy::Fresh));
        assert_eq!(refresh.complete(), None);
    }

    #[test]
    fn always_fresh_applies_to_scheduled_requests() {
        let refresh = RefreshCoordinator::new(true);
        assert_eq!(
            refresh.request(false),
            RefreshDecision::Execute(FetchPolicy::Fresh)
        );
    }
}
