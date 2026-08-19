//! CLI 명령을 애플리케이션 사용 사례에 연결한다.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};

use super::cli::{self, Cli};
use crate::adapters::presentation::{self, plain};
use crate::application::{
    AgentResult, FetchError, FetchPolicy, HistoryRepository, UsageApplication,
};
use crate::bootstrap::{self, Runtime};
use crate::domain::usage::UsageSnapshot;
use crate::local_timezone;

const MULTI_PROG: &str = "agentmeter";
const MULTI_ABOUT: &str = "설정한 에이전트들의 사용 한도를 한 화면에서 보여줍니다";

#[derive(Debug, Parser)]
#[command(name = MULTI_PROG, about = MULTI_ABOUT, version, long_about = None)]
struct Root {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    view: Cli,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 표시할 에이전트를 설정합니다
    Config(ConfigArgs),
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[command(subcommand)]
    action: ConfigAction,
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// 설정 전체를 보여줍니다
    List,
    /// 한 항목의 값을 보여줍니다
    Get { key: String },
    /// 값을 저장합니다 — `agents=claude,codex`
    Set { assignment: String },
}

pub(crate) fn main_agentmeter() -> ExitCode {
    let root = Root::parse();
    let runtime = match bootstrap::production() {
        Ok(runtime) => runtime,
        Err(error) => return report_error(MULTI_PROG, error),
    };
    match root.command {
        Some(Command::Config(arguments)) => match config_command(arguments.action, &runtime) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => report_error(MULTI_PROG, error),
        },
        None => match runtime.settings.load() {
            Ok(settings) => finish(
                MULTI_PROG,
                &root.view,
                runtime.usage,
                runtime.history,
                settings.agents,
            ),
            Err(error) => report_error(MULTI_PROG, error),
        },
    }
}

pub(crate) fn main_single(
    prog: &'static str,
    about: &'static str,
    agent_name: &'static str,
) -> ExitCode {
    let arguments = Cli::parse_for(prog, about);
    let runtime = match bootstrap::production() {
        Ok(runtime) => runtime,
        Err(error) => return report_error(prog, error),
    };
    finish(
        prog,
        &arguments,
        runtime.usage,
        runtime.history,
        vec![agent_name.to_string()],
    )
}

fn finish(
    prog: &'static str,
    arguments: &Cli,
    application: UsageApplication,
    history: Arc<dyn HistoryRepository>,
    names: Vec<String>,
) -> ExitCode {
    match run(prog, arguments, application, history, names) {
        Ok(code) => code,
        Err(error) => report_error(prog, error),
    }
}

fn report_error(prog: &str, error: anyhow::Error) -> ExitCode {
    eprintln!("{prog}: {error:#}");
    ExitCode::FAILURE
}

fn run(
    prog: &'static str,
    arguments: &Cli,
    application: UsageApplication,
    history: Arc<dyn HistoryRepository>,
    names: Vec<String>,
) -> Result<ExitCode> {
    // 이름 검증은 조회 전 애플리케이션 경계에서 한 번 수행한다.
    application.info(&names)?;
    let stdout_is_tty = std::io::stdout().is_terminal();
    let timezone = local_timezone();

    if arguments.json {
        let panes = fetch(&application, &names, arguments.live)?;
        println!("{}", presentation::to_json_panes(&panes, &timezone)?);
        return Ok(exit_for(&panes));
    }

    if arguments.is_watch() {
        if stdout_is_tty {
            if arguments.interval_was_clamped() {
                eprintln!(
                    "{prog}: 갱신 주기를 {}초로 올렸습니다 (원격 조회라 최소 {}초)",
                    arguments.interval_secs(),
                    cli::MIN_INTERVAL
                );
            }
            presentation::tui::run(
                prog,
                arguments.interval_secs(),
                timezone,
                application,
                history,
                names,
                arguments.live,
            )?;
            return Ok(ExitCode::SUCCESS);
        }
        eprintln!("{prog}: 출력이 터미널이 아니라 1회 출력합니다 (watch 와 함께 쓰세요)");
    }

    once(
        prog,
        arguments,
        stdout_is_tty,
        &application,
        &names,
        &timezone,
    )
}

fn fetch(application: &UsageApplication, names: &[String], live: bool) -> Result<Vec<AgentResult>> {
    application.query(
        names,
        if live {
            FetchPolicy::Fresh
        } else {
            FetchPolicy::PreferCached
        },
    )
}

fn once(
    prog: &str,
    arguments: &Cli,
    is_tty: bool,
    application: &UsageApplication,
    names: &[String],
    timezone: &str,
) -> Result<ExitCode> {
    let color = presentation::use_color(arguments.no_color, is_tty);
    let width = terminal_width();
    let panes = fetch(application, names, arguments.live)?;

    let (text, success) = if let [pane] = &panes[..] {
        once_output(prog, timezone, color, width, &pane.result)
    } else {
        (
            plain::render_panes(&panes, timezone, color, width),
            panes.iter().all(|pane| pane.result.is_ok()),
        )
    };

    write!(std::io::stdout().lock(), "{text}")?;
    Ok(if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn exit_for(results: &[AgentResult]) -> ExitCode {
    if results.iter().all(|result| result.result.is_ok()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn once_output(
    prog: &str,
    timezone: &str,
    color: bool,
    width: usize,
    result: &Result<UsageSnapshot, FetchError>,
) -> (String, bool) {
    match result {
        Ok(snapshot) => (plain::render(snapshot, timezone, color, width), true),
        Err(error) => (plain::render_error(prog, &error.to_string(), color), false),
    }
}

fn config_command(action: ConfigAction, runtime: &Runtime) -> Result<()> {
    match action {
        ConfigAction::List => {
            let current = runtime.settings.load()?;
            println!("agents = {}", current.agents.join(","));
            println!("# 설정 파일: {}", runtime.settings_path.display());
            Ok(())
        }
        ConfigAction::Get { key } => {
            let current = runtime.settings.load()?;
            match key.as_str() {
                "agents" => {
                    println!("{}", current.agents.join(","));
                    Ok(())
                }
                other => bail!("알 수 없는 설정 키: {other}. 쓸 수 있는 키: agents"),
            }
        }
        ConfigAction::Set { assignment } => {
            let (key, value) = split_assignment(&assignment)?;
            let current = match key {
                "agents" => runtime.settings.replace_agents(split_list(value))?,
                other => bail!("알 수 없는 설정 키: {other}. 쓸 수 있는 키: agents"),
            };
            println!("agents = {}", current.agents.join(","));
            println!("저장했습니다: {}", runtime.settings_path.display());
            Ok(())
        }
    }
}

fn split_assignment(argument: &str) -> Result<(&str, &str)> {
    match argument.split_once('=') {
        Some((key, value)) => Ok((key.trim(), value.trim())),
        None => bail!("`키=값` 형태여야 합니다. 예: agents=claude,codex"),
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn terminal_width() -> usize {
    crossterm::terminal::size()
        .map(|(width, _)| width as usize)
        .unwrap_or(80)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::usage::{UsageLimit, UsageSnapshot};

    fn snapshot() -> UsageSnapshot {
        UsageSnapshot::live(
            vec![UsageLimit::new(
                "session:all",
                None,
                50.0,
                None,
                false,
                Some(chrono::TimeDelta::hours(5)),
                None,
            )],
            chrono::Local::now(),
        )
    }

    #[test]
    fn success_renders_the_domain_snapshot() {
        let (text, success) = once_output("ccmeter", "Asia/Seoul", false, 80, &Ok(snapshot()));
        assert!(success);
        assert!(text.contains("Current session"));
        assert!(text.contains("50% used"));
    }

    #[test]
    fn failures_are_rendered_in_the_body() {
        let error = FetchError::Unauthorized("재로그인 필요".into());
        let (text, success) = once_output("ccmeter", "Asia/Seoul", false, 80, &Err(error));
        assert!(!success);
        assert!(text.contains("재로그인 필요"));
        assert!(text.starts_with("ccmeter:"));
    }

    #[test]
    fn config_syntax_belongs_to_the_inbound_adapter() {
        assert_eq!(
            split_assignment(" agents = claude,codex ").unwrap(),
            ("agents", "claude,codex")
        );
        assert_eq!(split_list("claude, codex,"), vec!["claude", "codex"]);
        assert!(split_assignment("agents").is_err());
    }
}
