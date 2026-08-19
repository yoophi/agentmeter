//! 사용량 조회가 실패했을 때 인바운드 어댑터에 전달하는 오류.

#[derive(Debug)]
pub enum FetchError {
    /// 재로그인이 필요한 상태. 상주 모드에서는 종료시키지 않고 화면에 띄운다.
    Unauthorized(String),
    Other(anyhow::Error),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Unauthorized(message) => write!(f, "{message}"),
            FetchError::Other(error) => write!(f, "{error:#}"),
        }
    }
}

impl std::error::Error for FetchError {}
