//! 로컬 웹 대시보드 인바운드 어댑터.
//!
//! 조회 상태는 `LiveSession`이 소유한다. 이 어댑터는 그 상태를 HTTP로 노출하고
//! 새로고침 요청을 세션에 전달하기만 한다.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Local;
use serde::Deserialize;
use tokio::sync::Notify;

use crate::adapters::presentation::web as presentation;
use crate::application::LiveSession;

/// 런타임 종료를 기다리는 상한. 조회가 진행 중이어도 프롬프트를 오래 붙잡지 않는다.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(200);

pub(crate) struct Options {
    pub session: Arc<LiveSession>,
    pub timezone: String,
    pub port: Option<u16>,
    pub host: IpAddr,
    /// TUI와 함께 뜨면 터미널이 화면에 쓰이므로 진단 출력을 억제한다.
    pub quiet: bool,
}

#[derive(Clone)]
struct WebState {
    session: Arc<LiveSession>,
    timezone: Arc<String>,
    quiet: bool,
}

impl WebState {
    fn report(&self, error: &dyn std::fmt::Display) {
        if !self.quiet {
            eprintln!("agentmeter web: {error}");
        }
    }
}

/// 백그라운드에서 도는 대시보드 서버. 주소는 바인딩 직후 확정된다.
pub(crate) struct Server {
    address: SocketAddr,
    shutdown: Arc<Notify>,
    thread: JoinHandle<Result<()>>,
}

impl Server {
    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    /// 종료를 요청하고 서버 스레드가 정리될 때까지 기다린다.
    pub(crate) fn shutdown(self) -> Result<()> {
        self.shutdown.notify_one();
        self.join()
    }

    /// Ctrl-C 처럼 서버가 스스로 끝낼 때까지 기다린다.
    pub(crate) fn wait(self) -> Result<()> {
        self.join()
    }

    fn join(self) -> Result<()> {
        match self.thread.join() {
            Ok(result) => result,
            Err(_) => bail!("웹 서버 스레드가 비정상 종료했습니다"),
        }
    }
}

/// 서버를 백그라운드 스레드에서 띄우고 확정된 주소를 돌려준다.
///
/// 바인딩 실패는 이 함수의 오류로 나온다 — 화면을 띄우기 전에 알아야 한다.
pub(crate) fn spawn(options: Options) -> Result<Server> {
    let shutdown = Arc::new(Notify::new());
    let signal = Arc::clone(&shutdown);
    let host = options.host;
    let port = options.port.unwrap_or(0);
    let state = WebState {
        session: options.session,
        timezone: Arc::new(options.timezone),
        quiet: options.quiet,
    };
    let (ready_tx, ready_rx) = mpsc::channel::<Result<SocketAddr>>();

    let thread = thread::Builder::new()
        .name("agentmeter-web".into())
        .spawn(move || serve_blocking(state, host, port, signal, ready_tx))
        .context("웹 서버 스레드를 만들 수 없습니다")?;

    match ready_rx.recv() {
        Ok(Ok(address)) => Ok(Server {
            address,
            shutdown,
            thread,
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            bail!("웹 서버가 주소를 알리지 못했습니다")
        }
    }
}

fn serve_blocking(
    state: WebState,
    host: IpAddr,
    port: u16,
    shutdown: Arc<Notify>,
    ready: mpsc::Sender<Result<SocketAddr>>,
) -> Result<()> {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return report_startup(
                &ready,
                Err(anyhow::Error::new(error).context("웹 서버 런타임을 만들 수 없습니다")),
            );
        }
    };

    let bound = runtime.block_on(bind(host, port));
    let (listener, address) = match bound {
        Ok(pair) => pair,
        Err(error) => return report_startup(&ready, Err(error)),
    };
    if ready.send(Ok(address)).is_err() {
        // 호출자가 사라졌으면 서버를 띄울 이유가 없다.
        return Ok(());
    }

    let served = runtime.block_on(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(shutdown_signal(shutdown))
            .await
            .context("웹 서버 실행 실패")
    });
    runtime.shutdown_timeout(SHUTDOWN_GRACE);
    served
}

/// 시작 실패는 호출자에게만 전하고 스레드 자체는 조용히 끝낸다.
fn report_startup(
    ready: &mpsc::Sender<Result<SocketAddr>>,
    result: Result<SocketAddr>,
) -> Result<()> {
    let _ = ready.send(result);
    Ok(())
}

async fn bind(host: IpAddr, port: u16) -> Result<(tokio::net::TcpListener, SocketAddr)> {
    let listener = tokio::net::TcpListener::bind((host, port))
        .await
        .with_context(|| {
            if port == 0 {
                format!("{host}에서 ephemeral port를 열 수 없습니다")
            } else {
                format!("{host}:{port} 포트를 열 수 없습니다")
            }
        })?;
    let address = listener
        .local_addr()
        .context("할당된 포트를 읽을 수 없습니다")?;
    Ok((listener, address))
}

fn router(state: WebState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/dashboard", get(dashboard))
        .route("/api/refresh", post(refresh))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    (
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Html(presentation::INDEX),
    )
}

async fn dashboard(State(state): State<WebState>) -> impl IntoResponse {
    let payload = {
        let current = state.session.read();
        presentation::project(
            &current.watch,
            &state.timezone,
            Local::now(),
            current.next_refresh_at,
            current.refreshing,
        )
    };
    Json(payload)
}

#[derive(Debug, Default, Deserialize)]
struct RefreshQuery {
    #[serde(default)]
    live: bool,
}

async fn refresh(
    State(state): State<WebState>,
    Query(query): Query<RefreshQuery>,
) -> impl IntoResponse {
    let session = Arc::clone(&state.session);
    // 조회는 blocking이라 런타임 워커를 막지 않도록 따로 돌린다.
    match tokio::task::spawn_blocking(move || session.refresh_blocking(query.live)).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT,
        Ok(Err(error)) => {
            state.report(&format!("{error:#}"));
            StatusCode::INTERNAL_SERVER_ERROR
        }
        Err(error) => {
            state.report(&format!("조회 작업이 중단되었습니다: {error}"));
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn shutdown_signal(shutdown: Arc<Notify>) {
    tokio::select! {
        _ = shutdown.notified() => {}
        result = tokio::signal::ctrl_c() => {
            if let Err(error) = result {
                eprintln!("agentmeter web: 종료 신호를 기다릴 수 없습니다: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use chrono::DateTime;

    use super::*;
    use crate::application::{
        AgentInfo, FetchError, FetchPolicy, HistoryRepository, HistoryRestore, RegisteredAgent,
        UsageApplication, UsageSource, WindowHistory,
    };
    use crate::domain::usage::UsageSnapshot;

    struct StubSource;

    impl UsageSource for StubSource {
        fn fetch(&self, _policy: FetchPolicy) -> Result<UsageSnapshot, FetchError> {
            Ok(UsageSnapshot::live(vec![], Local::now()))
        }
    }

    struct NoHistory;

    impl HistoryRepository for NoHistory {
        fn restore_active(&self, _provider: &str, _at: DateTime<Local>) -> Result<HistoryRestore> {
            Ok(HistoryRestore::default())
        }

        fn record(&self, _provider: &str, _snapshot: &UsageSnapshot) -> Result<Vec<WindowHistory>> {
            Ok(Vec::new())
        }
    }

    fn session() -> Arc<LiveSession> {
        let info = AgentInfo {
            name: "claude",
            display: "Claude Code",
        };
        let application =
            UsageApplication::new(vec![RegisteredAgent::new(info, StubSource)]).unwrap();
        Arc::new(LiveSession::new(
            application,
            vec!["claude".into()],
            vec![info],
            Arc::new(NoHistory),
            false,
        ))
    }

    fn options(session: Arc<LiveSession>) -> Options {
        Options {
            session,
            timezone: "Asia/Seoul".into(),
            port: None,
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            quiet: true,
        }
    }

    #[test]
    fn spawn_reports_the_bound_address_before_the_screen_starts() {
        let server = spawn(options(session())).unwrap();
        let address = server.address();
        assert_eq!(address.ip().to_string(), "127.0.0.1");
        assert_ne!(address.port(), 0);
        server.shutdown().unwrap();
    }

    #[test]
    fn a_taken_port_fails_before_the_screen_starts() {
        let held = spawn(options(session())).unwrap();
        let mut taken = options(session());
        taken.port = Some(held.address().port());
        let error = match spawn(taken) {
            Ok(_) => panic!("이미 쓰는 포트에는 바인딩되지 않아야 함"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("포트를 열 수 없습니다"),
            "{error:#}"
        );
        held.shutdown().unwrap();
    }

    #[test]
    fn the_screen_and_the_server_read_one_session() {
        let session = session();
        let server = spawn(options(Arc::clone(&session))).unwrap();
        session.refresh_blocking(false).unwrap();
        assert!(session.read().watch.panes()[0].snapshot.is_some());
        server.shutdown().unwrap();
    }

    #[test]
    fn dashboard_asset_is_self_contained() {
        assert!(presentation::INDEX.contains("<svg"));
        assert!(presentation::INDEX.contains("/api/dashboard"));
        assert!(presentation::INDEX.contains("--chart-rows: 3"));
        assert!(presentation::INDEX.contains("calc(var(--text-row) * var(--chart-rows))"));
        assert!(presentation::INDEX.contains("@media (max-width: 600px)"));
        assert!(presentation::INDEX.contains("setInterval(tickCountdowns, 1000)"));
        assert!(presentation::INDEX.contains("class=\"time-fill\""));
        assert!(presentation::INDEX.contains("elapsed * 100"));
        assert!(presentation::INDEX.contains("'midnight-line'"));
        assert!(presentation::INDEX.contains("'hour-line'"));
        assert!(presentation::INDEX.contains("class=\"error-toast\""));
        assert!(presentation::INDEX.contains("id=\"chrome-toggle\""));
        assert!(presentation::INDEX.contains("classList.toggle('chrome-hidden')"));
        assert!(
            presentation::INDEX.contains("width: 44px; height: 44px"),
            "모바일 토글은 손가락으로 누를 수 있는 touch target이어야 함"
        );
        assert!(presentation::INDEX.contains("env(safe-area-inset-top)"));
        assert!(
            presentation::INDEX
                .contains("setTimeout(() => errorToast.classList.remove('visible'), 3000)")
        );
        assert!(!presentation::INDEX.contains("https://"));
    }
}
