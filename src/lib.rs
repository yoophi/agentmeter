//! Agentmeter 실행 진입점. 내부 헥사곤은 바이너리 API로 노출하지 않는다.

mod adapters;
mod application;
mod bootstrap;
mod domain;

/// 통합 CLI를 실행한다.
pub fn run_agentmeter() -> std::process::ExitCode {
    adapters::inbound::runner::main_agentmeter()
}

fn local_timezone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "local".to_string())
}
