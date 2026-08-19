use std::process::ExitCode;

use agentmeter::app;

fn main() -> ExitCode {
    app::main_single("codexmeter", "Codex 사용 한도를 한눈에 보여줍니다", "codex")
}
