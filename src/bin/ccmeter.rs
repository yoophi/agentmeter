use std::process::ExitCode;

use agentmeter::app::{self, Fetch};
use agentmeter::claude;

fn main() -> ExitCode {
    app::main(
        "ccmeter",
        "Claude Code 사용 한도를 한눈에 보여줍니다",
        make_fetch,
    )
}

/// 기본은 Claude Code 가 남긴 로컬 캐시를 읽고,
/// 오래됐을 때만 직접 조회한다. `--live` 는 항상 직접 조회한다.
fn make_fetch(tz: String, live: bool) -> Fetch {
    Box::new(move || {
        if live {
            claude::source::fetch_live(&tz)
        } else {
            claude::source::fetch(&tz)
        }
    })
}
