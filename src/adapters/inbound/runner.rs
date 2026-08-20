//! CLI 명령을 애플리케이션 사용 사례에 연결한다.

use std::io::{IsTerminal, Write};
use std::net::{IpAddr, Ipv4Addr};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};

use super::cli::{self, Cli};
use super::web;
use crate::adapters::presentation::{self, plain};
use crate::application::{
    AgentResult, FetchError, FetchPolicy, HistoryRepository, LiveSession, UsageApplication,
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

    /// 설정과 관계없이 이번 실행에 표시할 에이전트
    #[arg(long, global = true, value_name = "NAME")]
    agent: Option<String>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 표시할 에이전트를 설정합니다
    Config(ConfigArgs),
    /// 브라우저에서 보는 로컬 실시간 대시보드를 실행합니다
    Web(WebArgs),
}

#[derive(Debug, Args)]
struct WebArgs {
    /// 캐시를 건너뛰고 매 주기 직접 조회합니다
    #[arg(long)]
    live: bool,

    /// 갱신 주기(초)
    #[arg(short = 'n', long, value_name = "SECS", default_value_t = cli::DEFAULT_INTERVAL)]
    interval: u64,

    /// 고정 포트로 실행합니다 (생략하면 ephemeral port)
    #[arg(long, value_name = "PORT")]
    port: Option<u16>,

    /// 서버가 바인딩할 IP 주소
    #[arg(long, value_name = "HOST", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    host: IpAddr,
}

impl WebArgs {
    fn interval_secs(&self) -> u64 {
        self.interval.max(cli::MIN_INTERVAL)
    }
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
        Some(Command::Web(arguments)) => match selected_agent_names(root.agent.as_deref(), || {
            runtime.settings.load().map(|settings| settings.agents)
        }) {
            Ok(names) => match web_command(arguments, runtime, names) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => report_error(MULTI_PROG, error),
            },
            Err(error) => report_error(MULTI_PROG, error),
        },
        None => match selected_agent_names(root.agent.as_deref(), || {
            runtime.settings.load().map(|settings| settings.agents)
        }) {
            Ok(names) => finish(
                MULTI_PROG,
                &root.view,
                runtime.usage,
                runtime.history,
                names,
            ),
            Err(error) => report_error(MULTI_PROG, error),
        },
    }
}

fn selected_agent_names(
    agent: Option<&str>,
    load_configured: impl FnOnce() -> Result<Vec<String>>,
) -> Result<Vec<String>> {
    match agent {
        Some(name) => Ok(vec![name.to_string()]),
        None => load_configured(),
    }
}

/// 웹 서버는 배경에서 돌리고 터미널에는 상주 화면을 띄운다.
///
/// 두 화면이 같은 `LiveSession`을 보므로 provider 조회는 한 번만 나간다.
fn web_command(arguments: WebArgs, runtime: Runtime, names: Vec<String>) -> Result<()> {
    if arguments.interval < cli::MIN_INTERVAL {
        eprintln!(
            "{MULTI_PROG}: 갱신 주기를 {}초로 올렸습니다 (원격 조회라 최소 {}초)",
            arguments.interval_secs(),
            cli::MIN_INTERVAL
        );
    }
    let agents = runtime.usage.info(&names)?;
    let timezone = local_timezone();
    let session = Arc::new(LiveSession::new(
        runtime.usage,
        names,
        agents,
        runtime.history,
        arguments.live,
    ));
    spawn_refresh_loop(&session, arguments.interval_secs());

    // 터미널이 화면에 쓰이면 서버는 stdout·stderr를 건드릴 수 없다.
    let with_screen = std::io::stdout().is_terminal();
    let server = web::spawn(web::Options {
        session: Arc::clone(&session),
        timezone: timezone.clone(),
        port: arguments.port,
        host: arguments.host,
        quiet: with_screen,
    })?;
    let address = format!("http://{}", server.address());

    if with_screen {
        presentation::tui::run(MULTI_PROG, timezone, session, Some(address))?;
        server.shutdown()
    } else {
        println!("{MULTI_PROG} web: {address}");
        println!("종료하려면 Ctrl-C를 누르세요.");
        server.wait()
    }
}

/// 세션의 주기 조회를 전용 스레드에서 돌린다. 세션마다 한 번만 호출한다.
fn spawn_refresh_loop(session: &Arc<LiveSession>, interval_secs: u64) {
    let session = Arc::clone(session);
    let interval = Duration::from_secs(interval_secs);
    thread::spawn(move || session.run_refresh_loop(interval));
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
    let agents = application.info(&names)?;
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
            let session = Arc::new(LiveSession::new(
                application,
                names,
                agents,
                history,
                arguments.live,
            ));
            spawn_refresh_loop(&session, arguments.interval_secs());
            presentation::tui::run(prog, timezone, session, None)?;
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
        let (text, success) = once_output("agentmeter", "Asia/Seoul", false, 80, &Ok(snapshot()));
        assert!(success);
        assert!(text.contains("Current session"));
        assert!(text.contains("50% used"));
    }

    #[test]
    fn failures_are_rendered_in_the_body() {
        let error = FetchError::Unauthorized("재로그인 필요".into());
        let (text, success) = once_output("agentmeter", "Asia/Seoul", false, 80, &Err(error));
        assert!(!success);
        assert!(text.contains("재로그인 필요"));
        assert!(text.starts_with("agentmeter:"));
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

    #[test]
    fn explicit_agent_overrides_configuration_without_loading_it() {
        let names = selected_agent_names(Some("claude"), || {
            panic!("명시적 agent가 있으면 설정을 읽지 않아야 함")
        })
        .unwrap();
        assert_eq!(names, vec!["claude"]);
    }

    #[test]
    fn absent_agent_uses_configuration() {
        let names =
            selected_agent_names(None, || Ok(vec!["codex".into(), "claude".into()])).unwrap();
        assert_eq!(names, vec!["codex", "claude"]);
    }

    #[test]
    fn agent_override_combines_with_watch_and_json_modes() {
        let watch = Root::try_parse_from(["agentmeter", "--agent", "claude", "--watch"]).unwrap();
        assert_eq!(watch.agent.as_deref(), Some("claude"));
        assert!(watch.view.watch);

        let json = Root::try_parse_from(["agentmeter", "--agent", "codex", "--json"]).unwrap();
        assert_eq!(json.agent.as_deref(), Some("codex"));
        assert!(json.view.json);
    }

    #[test]
    fn web_interval_uses_the_same_remote_floor() {
        assert_eq!(
            WebArgs {
                live: false,
                interval: 1,
                port: None,
                host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            }
            .interval_secs(),
            cli::MIN_INTERVAL
        );
    }

    #[test]
    fn web_is_an_explicit_subcommand() {
        let root = Root::try_parse_from([
            "agentmeter",
            "web",
            "--agent",
            "claude",
            "--live",
            "--interval",
            "90",
            "--port",
            "8080",
            "--host",
            "0.0.0.0",
        ])
        .unwrap();
        assert_eq!(root.agent.as_deref(), Some("claude"));
        let Some(Command::Web(arguments)) = root.command else {
            panic!("web subcommand를 파싱해야 함");
        };
        assert!(arguments.live);
        assert_eq!(arguments.interval_secs(), 90);
        assert_eq!(arguments.port, Some(8080));
        assert_eq!(arguments.host, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn web_host_defaults_to_ipv4_loopback() {
        let root = Root::try_parse_from(["agentmeter", "web"]).unwrap();
        let Some(Command::Web(arguments)) = root.command else {
            panic!("web subcommand를 파싱해야 함");
        };
        assert_eq!(arguments.host, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
}
