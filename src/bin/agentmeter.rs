//! 설정에 적힌 에이전트를 한 화면에서 보여준다.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

use agentmeter::cli::Cli;
use agentmeter::{app, config, registry};

const PROG: &str = "agentmeter";
const ABOUT: &str = "설정한 에이전트들의 사용 한도를 한 화면에서 보여줍니다";

#[derive(Debug, Parser)]
#[command(name = PROG, about = ABOUT, version, long_about = None)]
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
    /// 한 항목의 값을 보여줍니다 — `agentmeter config get agents`
    Get {
        /// 설정 키 (현재는 `agents`)
        key: String,
    },
    /// 값을 저장합니다 — `agentmeter config set agents=claude,codex`
    Set {
        /// `키=값` — 목록은 쉼표로 구분합니다
        assignment: String,
    },
}

fn main() -> ExitCode {
    let root = Root::parse();
    match root.command {
        Some(Command::Config(args)) => match config_command(args.action) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{PROG}: {e:#}");
                ExitCode::FAILURE
            }
        },
        None => view(&root.view),
    }
}

fn view(args: &Cli) -> ExitCode {
    let cfg = match config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{PROG}: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let specs: Vec<_> = cfg
        .agents
        .iter()
        .filter_map(|n| registry::find(n))
        .collect();
    if specs.is_empty() {
        eprintln!(
            "{PROG}: 표시할 에이전트가 없습니다. `{PROG} config set agents={}` 로 설정하세요",
            registry::names().join(",")
        );
        return ExitCode::FAILURE;
    }
    app::main_multi(PROG, args, specs)
}

fn config_command(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::List => {
            let cfg = config::load()?;
            println!("agents = {}", cfg.agents.join(","));
            println!("# 설정 파일: {}", config::path()?.display());
            Ok(())
        }
        ConfigAction::Get { key } => {
            let cfg = config::load()?;
            match key.as_str() {
                "agents" => {
                    println!("{}", cfg.agents.join(","));
                    Ok(())
                }
                other => anyhow::bail!("알 수 없는 설정 키: {other}. 쓸 수 있는 키: agents"),
            }
        }
        ConfigAction::Set { assignment } => {
            let (key, value) = config::split_assignment(&assignment)?;
            let mut cfg = config::load().unwrap_or_default();
            match key {
                "agents" => cfg.agents = config::split_list(value),
                other => anyhow::bail!("알 수 없는 설정 키: {other}. 쓸 수 있는 키: agents"),
            }
            // 저장하기 전에 검증한다 — 잘못된 값을 파일에 남기면 다음 실행이 죽는다
            config::validate(&cfg)?;
            let path = config::save(&cfg)?;
            println!("agents = {}", cfg.agents.join(","));
            println!("저장했습니다: {}", path.display());
            Ok(())
        }
    }
}
