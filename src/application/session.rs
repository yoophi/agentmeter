//! TUI와 웹이 함께 보는 하나의 상주 조회 세션.
//!
//! 화면이 둘이어도 provider 조회는 한 번만 나가야 한다. 세션이 조회 상태를
//! 소유하고, 화면 어댑터는 그 상태를 읽어 그리기만 한다.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
use std::time::Duration;

use chrono::{DateTime, Local, TimeDelta};

use super::{
    AgentInfo, FetchPolicy, HistoryRepository, RefreshCoordinator, RefreshDecision,
    UsageApplication, WatchState,
};

/// 화면 어댑터가 한 프레임을 그리기 위해 읽는 상태.
pub(crate) struct SessionState {
    pub watch: WatchState,
    pub refreshing: bool,
    pub next_refresh_at: Option<DateTime<Local>>,
}

impl SessionState {
    /// 다음 자동 조회까지 남은 초. 이미 지났으면 0으로 붙인다.
    pub(crate) fn seconds_until_refresh(&self, now: DateTime<Local>) -> Option<u64> {
        let remaining = self.next_refresh_at? - now;
        Some(remaining.num_seconds().max(0) as u64)
    }
}

pub(crate) struct LiveSession {
    application: UsageApplication,
    names: Vec<String>,
    refresh: RefreshCoordinator,
    state: RwLock<SessionState>,
    /// 조회 루프를 깨우는 요청 큐. 루프가 하나뿐이라 Mutex로 충분하다.
    requests: Mutex<Receiver<FetchPolicy>>,
    sender: Sender<FetchPolicy>,
}

impl LiveSession {
    pub(crate) fn new(
        application: UsageApplication,
        names: Vec<String>,
        agents: Vec<AgentInfo>,
        history: Arc<dyn HistoryRepository>,
        live: bool,
    ) -> Self {
        Self::with_watch(
            application,
            names,
            WatchState::persistent(agents, history),
            live,
        )
    }

    fn with_watch(
        application: UsageApplication,
        names: Vec<String>,
        watch: WatchState,
        live: bool,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            application,
            names,
            refresh: RefreshCoordinator::new(live),
            state: RwLock::new(SessionState {
                watch,
                refreshing: false,
                next_refresh_at: None,
            }),
            requests: Mutex::new(receiver),
            sender,
        }
    }

    pub(crate) fn read(&self) -> RwLockReadGuard<'_, SessionState> {
        self.state.read().expect("session state lock")
    }

    /// 조회를 요청한다. 실행은 조회 루프가 맡으므로 호출자는 막히지 않는다.
    ///
    /// 키 입력처럼 화면을 멈출 수 없는 자리에서 쓴다.
    pub(crate) fn request(&self, force_live: bool) {
        if let RefreshDecision::Execute(policy) = self.refresh.request(force_live) {
            // Receiver를 세션이 들고 있어서 send는 실패하지 않는다. 루프가 아직
            // 시작되지 않았으면 큐에 남아 첫 대기 지점에서 소비된다.
            let _ = self.sender.send(policy);
        }
    }

    /// 호출한 스레드에서 조회를 끝까지 실행한다.
    ///
    /// 이미 조회 중이면 요청만 병합하고 즉시 반환한다. 요청-응답으로 결과를
    /// 돌려줘야 하는 HTTP 핸들러가 쓴다.
    pub(crate) fn refresh_blocking(&self, force_live: bool) -> anyhow::Result<()> {
        let RefreshDecision::Execute(policy) = self.refresh.request(force_live) else {
            return Ok(());
        };
        match self.drive(policy) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// 주기 조회 루프. 전용 스레드 하나에서만 돌려야 한다.
    pub(crate) fn run_refresh_loop(&self, interval: Duration) {
        let mut policy = match self.refresh.request(false) {
            RefreshDecision::Execute(policy) => policy,
            RefreshDecision::Queued => unreachable!("새 세션의 첫 요청은 즉시 실행된다"),
        };
        loop {
            // provider별 실패는 pane 상태로 이미 남으므로 여기서는 삼킨다.
            let _ = self.drive(policy);
            self.schedule_next(interval);
            match self.wait(interval) {
                Some(requested) => policy = requested,
                None => return,
            }
        }
    }

    /// 요청이 오거나 주기가 지날 때까지 기다린다. 큐가 끊기면 `None`.
    fn wait(&self, interval: Duration) -> Option<FetchPolicy> {
        let requests = self.requests.lock().expect("refresh request queue lock");
        loop {
            match requests.recv_timeout(interval) {
                Ok(requested) => return Some(requested),
                Err(RecvTimeoutError::Timeout) => match self.refresh.request(false) {
                    RefreshDecision::Execute(policy) => return Some(policy),
                    // 다른 곳이 조회 중이면 그 라운드가 끝날 때까지 더 기다린다.
                    RefreshDecision::Queued => continue,
                },
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// 병합된 요청까지 이어서 실행하고 첫 오류를 돌려준다.
    fn drive(&self, mut policy: FetchPolicy) -> Option<anyhow::Error> {
        self.set_refreshing(true);
        let mut first_error = None;
        loop {
            match self.application.query(&self.names, policy) {
                Ok(results) => self
                    .state
                    .write()
                    .expect("session state lock")
                    .watch
                    .apply(results),
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
            match self.refresh.complete() {
                Some(pending) => policy = pending,
                None => break,
            }
        }
        self.set_refreshing(false);
        first_error
    }

    fn set_refreshing(&self, refreshing: bool) {
        self.state.write().expect("session state lock").refreshing = refreshing;
    }

    fn schedule_next(&self, interval: Duration) {
        let at = Local::now() + TimeDelta::seconds(interval.as_secs() as i64);
        self.state
            .write()
            .expect("session state lock")
            .next_refresh_at = Some(at);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::application::{FetchError, RegisteredAgent, UsageSource};
    use crate::domain::usage::UsageSnapshot;

    struct CountingSource(Arc<AtomicUsize>);

    impl UsageSource for CountingSource {
        fn fetch(&self, _policy: FetchPolicy) -> Result<UsageSnapshot, FetchError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(UsageSnapshot::live(vec![], Local::now()))
        }
    }

    fn info() -> AgentInfo {
        AgentInfo {
            name: "claude",
            display: "Claude Code",
        }
    }

    fn session(calls: &Arc<AtomicUsize>) -> LiveSession {
        let application = UsageApplication::new(vec![RegisteredAgent::new(
            info(),
            CountingSource(Arc::clone(calls)),
        )])
        .unwrap();
        LiveSession::with_watch(
            application,
            vec!["claude".into()],
            WatchState::new(vec![info()]),
            false,
        )
    }

    #[test]
    fn one_session_serves_both_screens_from_a_single_fetch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let session = session(&calls);
        session.refresh_blocking(false).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(session.read().watch.panes()[0].snapshot.is_some());
    }

    #[test]
    fn a_request_during_a_fetch_is_merged_not_duplicated() {
        let calls = Arc::new(AtomicUsize::new(0));
        let session = session(&calls);
        // 조회를 하나 점유한 상태에서 들어온 요청은 대기열로 병합된다.
        session.request(false);
        session.request(false);
        session.refresh_blocking(true).unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "이미 점유된 요청은 루프가 집어가고, blocking 호출은 즉시 반환한다"
        );
    }

    #[test]
    fn refreshing_is_false_once_a_round_finishes() {
        let calls = Arc::new(AtomicUsize::new(0));
        let session = session(&calls);
        session.refresh_blocking(false).unwrap();
        assert!(!session.read().refreshing);
    }

    #[test]
    fn countdown_never_goes_negative() {
        let now = Local::now();
        let state = SessionState {
            watch: WatchState::new(vec![info()]),
            refreshing: false,
            next_refresh_at: Some(now - TimeDelta::seconds(5)),
        };
        assert_eq!(state.seconds_until_refresh(now), Some(0));
    }
}
