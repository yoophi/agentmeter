use clap::Parser;

#[derive(Debug, Parser)]
#[command(version = crate::VERSION, long_about = None)]
pub struct Cli {
    /// Stay resident and keep refreshing full screen
    #[arg(short = 'w', long)]
    pub watch: bool,

    /// Refresh interval in seconds. Implies watch mode
    #[arg(short = 'n', long, value_name = "SECS")]
    pub interval: Option<u64>,

    /// Print JSON, for statuslines and scripts
    #[arg(short = 'j', long, conflicts_with_all = ["watch", "interval"])]
    pub json: bool,

    /// Skip the local cache and fetch live
    #[arg(long)]
    pub live: bool,

    /// Disable colour
    #[arg(long)]
    pub no_color: bool,
}

pub const DEFAULT_INTERVAL: u64 = 60;
/// 원격 조회라서 짧은 주기는 의미가 없고 rate limit 만 소모한다.
pub const MIN_INTERVAL: u64 = 30;

impl Cli {
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
