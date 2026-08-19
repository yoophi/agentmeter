use clap::{CommandFactory, FromArgMatches, Parser};

#[derive(Debug, Parser)]
#[command(version, long_about = None)]
pub struct Cli {
    /// 상주 모드 — 전체 화면으로 계속 갱신합니다
    #[arg(short = 'w', long)]
    pub watch: bool,

    /// 갱신 주기(초). 지정하면 상주 모드가 켜집니다
    #[arg(short = 'n', long, value_name = "SECS")]
    pub interval: Option<u64>,

    /// JSON 으로 출력합니다 (statusline·스크립트용)
    #[arg(short = 'j', long, conflicts_with_all = ["watch", "interval"])]
    pub json: bool,

    /// 로컬 캐시를 건너뛰고 직접 조회합니다 (ccmeter 전용)
    #[arg(long)]
    pub live: bool,

    /// 색을 사용하지 않습니다
    #[arg(long)]
    pub no_color: bool,
}

pub const DEFAULT_INTERVAL: u64 = 60;
/// 원격 조회라서 짧은 주기는 의미가 없고 rate limit 만 소모한다.
pub const MIN_INTERVAL: u64 = 30;

impl Cli {
    /// 두 도구가 같은 옵션을 쓰되 이름과 설명만 바꿔 단다.
    pub fn parse_for(name: &'static str, about: &'static str) -> Self {
        let cmd = Self::command().name(name).about(about);
        Self::from_arg_matches(&cmd.get_matches()).unwrap_or_else(|e| e.exit())
    }

    /// `--watch` 또는 `--interval` 중 하나라도 있으면 상주 모드.
    pub fn is_watch(&self) -> bool {
        self.watch || self.interval.is_some()
    }

    /// 사용자가 준 값을 하한선으로 잘라낸 실제 주기.
    pub fn interval_secs(&self) -> u64 {
        self.interval.unwrap_or(DEFAULT_INTERVAL).max(MIN_INTERVAL)
    }

    /// 요청값이 하한선에 걸렸으면 알려주기 위해.
    pub fn interval_was_clamped(&self) -> bool {
        matches!(self.interval, Some(v) if v < MIN_INTERVAL)
    }
}
