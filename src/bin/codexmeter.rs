use std::process::ExitCode;

use agentmeter::app::{self, Fetch};
use agentmeter::codex;

fn main() -> ExitCode {
    app::main(
        "codexmeter",
        "Codex 사용 한도를 한눈에 보여줍니다",
        make_fetch,
    )
}

/// Codex 는 app-server 가 값을 들고 있어 호출이 저렴하다.
/// 캐시 계층이 없으므로 `--live` 여부와 무관하게 매번 직접 조회한다.
fn make_fetch(tz: String, _live: bool) -> Fetch {
    Box::new(move || codex::source::fetch(&tz))
}
