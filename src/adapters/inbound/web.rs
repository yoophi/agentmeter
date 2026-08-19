//! 로컬 웹 대시보드 인바운드 어댑터.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Local, TimeDelta};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::adapters::presentation::web as presentation;
use crate::application::{
    HistoryRepository, RefreshCoordinator, RefreshDecision, UsageApplication, WatchState,
};

pub(crate) struct Options {
    pub application: UsageApplication,
    pub history: Arc<dyn HistoryRepository>,
    pub names: Vec<String>,
    pub timezone: String,
    pub interval_secs: u64,
    pub port: Option<u16>,
    pub host: IpAddr,
    pub live: bool,
}

struct DashboardState {
    watch: WatchState,
    refreshing: bool,
    next_refresh_at: Option<chrono::DateTime<Local>>,
}

#[derive(Clone)]
struct WebState {
    application: UsageApplication,
    names: Arc<Vec<String>>,
    timezone: Arc<String>,
    interval: Duration,
    dashboard: Arc<RwLock<DashboardState>>,
    refresh: Arc<RefreshCoordinator>,
}

pub(crate) fn run(options: Options) -> anyhow::Result<()> {
    let agents = options.application.info(&options.names)?;
    let port = options.port.unwrap_or(0);
    let host = options.host;
    let state = WebState {
        application: options.application,
        names: Arc::new(options.names),
        timezone: Arc::new(options.timezone),
        interval: Duration::from_secs(options.interval_secs),
        dashboard: Arc::new(RwLock::new(DashboardState {
            watch: WatchState::persistent(agents, options.history),
            refreshing: false,
            next_refresh_at: None,
        })),
        refresh: Arc::new(RefreshCoordinator::new(options.live)),
    };

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("웹 서버 런타임을 만들 수 없습니다")?
        .block_on(serve(state, host, port))
}

async fn serve(state: WebState, host: IpAddr, port: u16) -> anyhow::Result<()> {
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
    let app = router(state.clone());

    println!("agentmeter web: http://{address}");
    println!("종료하려면 Ctrl-C를 누르세요.");

    tokio::spawn(refresh_loop(state));
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("웹 서버 실행 실패")
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
    let current = state.dashboard.read().await;
    Json(presentation::project(
        &current.watch,
        &state.timezone,
        Local::now(),
        current.next_refresh_at,
        current.refreshing,
    ))
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
    match refresh_once(&state, query.live).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            eprintln!("agentmeter web: {error:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn refresh_loop(state: WebState) {
    loop {
        if let Err(error) = refresh_once(&state, false).await {
            eprintln!("agentmeter web: {error:#}");
        }
        {
            let mut dashboard = state.dashboard.write().await;
            dashboard.next_refresh_at =
                Some(Local::now() + TimeDelta::seconds(state.interval.as_secs() as i64));
        }
        tokio::time::sleep(state.interval).await;
    }
}

async fn refresh_once(state: &WebState, force_live: bool) -> anyhow::Result<()> {
    let RefreshDecision::Execute(mut policy) = state.refresh.request(force_live) else {
        return Ok(());
    };
    state.dashboard.write().await.refreshing = true;
    let mut first_error = None;

    loop {
        let application = state.application.clone();
        let names = Arc::clone(&state.names);
        let result = tokio::task::spawn_blocking(move || application.query(&names, policy))
            .await
            .context("조회 작업이 중단되었습니다")
            .and_then(|result| result);
        match result {
            Ok(results) => state.dashboard.write().await.watch.apply(results),
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }

        match state.refresh.complete() {
            Some(pending) => policy = pending,
            None => break,
        }
    }
    state.dashboard.write().await.refreshing = false;
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("agentmeter web: 종료 신호를 기다릴 수 없습니다: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_an_ephemeral_loopback_port() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let address = runtime.block_on(async {
            tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap()
                .local_addr()
                .unwrap()
        });
        assert_eq!(address.ip().to_string(), "127.0.0.1");
        assert_ne!(address.port(), 0);
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
