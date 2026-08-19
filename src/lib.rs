pub mod app;
pub mod claude;
pub mod cli;
pub mod codex;
pub mod config;
pub mod history;
pub mod meter;
pub mod multi;
pub mod registry;
pub mod render;

/// 두 도구가 공유하는 조회 실패 표현.
#[derive(Debug)]
pub enum FetchError {
    /// 재로그인이 필요한 상태. 상주 모드에서 종료시키지 않고 화면에 띄운다.
    Unauthorized(String),
    Other(anyhow::Error),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Unauthorized(m) => write!(f, "{m}"),
            FetchError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for FetchError {}

pub fn local_tz() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "local".to_string())
}
