//! 실행 로직 — 모드 분기, 출력, 종료 코드.
//!
//! 단일 에이전트 도구(`ccmeter`, `codexmeter`)와 통합 도구(`agentmeter`)가
//! 같은 경로를 쓴다. 차이는 다룰 에이전트가 몇 개인지뿐이다.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use anyhow::Result;

use crate::cli::{self, Cli};
use crate::meter::Snapshot;
use crate::multi;
use crate::registry::{self, AgentSpec};
use crate::render::{self, plain};
use crate::{FetchError, local_tz};

/// 한 번 조회해 화면 표현을 만든다. 상주 모드에서는 워커 스레드가 반복 호출한다.
pub type Fetch = Box<dyn Fn(bool) -> Result<Snapshot, FetchError> + Send>;

/// 단일 에이전트 전용 진입점. `agent` 는 [`registry`] 의 이름이다.
pub fn main_single(prog: &'static str, about: &'static str, agent: &'static str) -> ExitCode {
    let args = Cli::parse_for(prog, about);
    let specs = match registry::find(agent) {
        Some(spec) => vec![spec],
        None => {
            eprintln!("{prog}: 등록되지 않은 에이전트입니다: {agent}");
            return ExitCode::FAILURE;
        }
    };
    finish(prog, &args, specs)
}

/// 설정에 적힌 에이전트를 모두 보여주는 진입점.
pub fn main_multi(prog: &'static str, args: &Cli, agents: Vec<&'static AgentSpec>) -> ExitCode {
    finish(prog, args, agents)
}

fn finish(prog: &'static str, args: &Cli, specs: Vec<&'static AgentSpec>) -> ExitCode {
    match run(prog, args, specs) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{prog}: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(prog: &'static str, args: &Cli, specs: Vec<&'static AgentSpec>) -> Result<ExitCode> {
    let stdout_is_tty = std::io::stdout().is_terminal();
    // 시간대 해석은 OS 호출이라 프로세스당 한 번만 한다
    let tz = local_tz();

    if args.json {
        let panes = fetch(&specs, &tz, args.live);
        println!("{}", render::to_json_panes(&panes)?);
        return Ok(if panes.iter().all(|p| p.result.is_ok()) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }

    // 상주 모드는 TTY 에서만 의미가 있다. ratatui 는 alternate screen 을 쓰므로
    // `watch` 아래나 파이프에서는 동작하지 않는다 — 그 경우 1회 출력으로 내려간다.
    if args.is_watch() {
        if stdout_is_tty {
            if args.interval_was_clamped() {
                eprintln!(
                    "{prog}: 갱신 주기를 {}초로 올렸습니다 (원격 조회라 최소 {}초)",
                    args.interval_secs(),
                    cli::MIN_INTERVAL
                );
            }
            render::tui::run(prog, args.interval_secs(), tz, specs, args.live)?;
            return Ok(ExitCode::SUCCESS);
        }
        eprintln!("{prog}: 출력이 터미널이 아니라 1회 출력합니다 (watch 와 함께 쓰세요)");
    }

    once(args, stdout_is_tty, &specs, &tz)
}

fn fetch(specs: &[&'static AgentSpec], tz: &str, live: bool) -> Vec<multi::Pane> {
    let names: Vec<String> = specs.iter().map(|s| s.name.to_string()).collect();
    multi::fetch_all(&names, tz, live, false)
}

fn once(args: &Cli, is_tty: bool, specs: &[&'static AgentSpec], tz: &str) -> Result<ExitCode> {
    let color = render::use_color(args.no_color, is_tty);
    let width = terminal_width();
    let panes = fetch(specs, tz, args.live);

    // 에이전트가 하나면 머리글이 군더더기다 — 기존 단일 도구 화면을 그대로 유지한다.
    let (text, ok) = if let [pane] = &panes[..] {
        once_output(pane.agent.binary, color, width, &pane.result)
    } else {
        (
            plain::render_panes(&panes, color, width),
            panes.iter().all(|p| p.result.is_ok()),
        )
    };

    write!(std::io::stdout().lock(), "{text}")?;
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// 1회 출력의 본문과 성공 여부.
///
/// 실패도 **stdout** 으로 나간다. `watch` 는 stdout 만 캡처하므로
/// stderr 로 보내면 오류가 났을 때 화면이 빈 채로 남는다.
fn once_output(
    prog: &str,
    color: bool,
    width: usize,
    result: &Result<Snapshot, FetchError>,
) -> (String, bool) {
    match result {
        Ok(snap) => (plain::render(snap, color, width), true),
        Err(e) => (plain::render_error(prog, &e.to_string(), color), false),
    }
}

fn terminal_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meter::Level;

    fn one() -> Snapshot {
        Snapshot::live(vec![crate::meter::Meter {
            title: "Current session".into(),
            usage: crate::meter::Bar {
                fill: 0.5,
                label: "50% used".into(),
                level: Level::Normal,
            },
            window: None,
            time: None,
            footnote: Some("Resets Aug 18 at 9:30pm (Asia/Seoul)".into()),
            emphasized: false,
        }])
    }

    #[test]
    fn success_renders_the_meters() {
        let (text, ok) = once_output("ccmeter", false, 80, &Ok(one()));
        assert!(ok);
        assert!(text.contains("Current session"));
        assert!(text.contains("50% used"));
    }

    /// 어떤 실패든 본문(stdout)에 담겨야 한다.
    /// stderr 로 새면 `watch` 화면이 빈 채로 남는다.
    #[test]
    fn every_failure_goes_into_the_body() {
        let cases = [
            FetchError::Unauthorized("재로그인 필요".into()),
            FetchError::Other(anyhow::anyhow!("조회 요청이 너무 잦습니다 (HTTP 429)")),
        ];
        for err in cases {
            let expect = err.to_string();
            let (text, ok) = once_output("ccmeter", false, 80, &Err(err));
            assert!(!ok, "실패는 실패 코드여야 함");
            assert!(text.contains(&expect), "본문에 사유가 있어야 함: {text}");
            assert!(
                text.starts_with("ccmeter:"),
                "프로그램 이름으로 시작: {text}"
            );
        }
    }
}
