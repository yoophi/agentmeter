use std::process::ExitCode;

use agentmeter::app;

fn main() -> ExitCode {
    app::main_single(
        "ccmeter",
        "Claude Code 사용 한도를 한눈에 보여줍니다",
        "claude",
    )
}
