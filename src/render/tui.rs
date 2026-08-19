//! 상주 모드 TUI.
//!
//! 화면은 에이전트마다 하나의 **구획(pane)** 으로 좌우로 나뉜다
//! (`A | B` — tmux 의 수직 분할과 같다). 단일 에이전트 도구
//! (`ccmeter`, `codexmeter`)는 구획이 하나인 특수한 경우이므로 코드가 하나다.
//!
//! 조회는 워커 스레드가 담당한다. 메인 스레드가 직접 하면 타임아웃 동안
//! 화면이 얼어붙고 키 입력도 안 먹는다.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::history::History;
use crate::meter::{Bar, Level, Meter, Origin};
use crate::multi;
use crate::registry::AgentSpec;

/// 유휴 시 화면을 다시 그리는 주기. 가장 잘게 변하는 것이 `다음 N초` 라 1초면 충분하다.
/// 키 입력은 `event::poll` 이 즉시 깨우므로 이 값과 무관하게 바로 반응한다.
const TICK: Duration = Duration::from_secs(1);

/// 게이지·차트·각주가 제목 아래에서 들여쓰는 칸 수.
const GAUGE_INDENT: u16 = 3;

/// 좌우로 놓인 구획 사이 여백.
const PANE_GAP: u16 = 2;

/// 화면 오른쪽 여백. 게이지가 터미널 경계에 붙으면 답답하고,
/// 창 크기를 줄일 때 글자가 잘리는 것처럼 보인다.
const RIGHT_MARGIN: u16 = 2;

/// 한 항목이 차지하는 줄 수 — 제목·사용량은 항상, 나머지는 있을 때만.
fn rows_for(m: &Meter, has_chart: bool) -> usize {
    2 + usize::from(m.time.is_some())
        + usize::from(has_chart)
        + usize::from(m.footnote.is_some())
        + 1 // 항목 사이 여백
}

/// 워커가 한 번 조회해 보낸 결과 — 구획 순서와 같다.
struct Round(Vec<PaneResult>);

struct PaneResult {
    agent: &'static AgentSpec,
    outcome: Result<Vec<Meter>, String>,
    origin: Option<Origin>,
}

struct Pane {
    agent: &'static AgentSpec,
    meters: Vec<Meter>,
    /// 값을 언제 어디서 가져왔는지 (캐시일 수 있다)
    origin: Option<Origin>,
    /// 앱을 켠 뒤로 모은 변화
    history: History,
    error: Option<String>,
}

impl Pane {
    fn new(agent: &'static AgentSpec) -> Self {
        Pane {
            agent,
            meters: Vec::new(),
            origin: None,
            history: History::default(),
            error: None,
        }
    }
}

struct App {
    prog: String,
    tz: String,
    panes: Vec<Pane>,
    next_fetch: Option<Instant>,
    interval: Duration,
    /// 구획 머리글을 붙일지. 에이전트가 하나면 군더더기다.
    label_panes: bool,
}

impl App {
    fn seconds_until_refresh(&self) -> Option<u64> {
        let next = self.next_fetch?;
        Some(next.saturating_duration_since(Instant::now()).as_secs())
    }

    /// 화면에 표시할 값의 기준 시각. 구획이 여럿이면 가장 오래된 것을 쓴다 —
    /// "이 화면 전체가 최소 이만큼 낡았다" 가 사용자가 알아야 할 값이다.
    fn oldest_origin(&self) -> Option<Origin> {
        self.panes
            .iter()
            .filter_map(|p| p.origin)
            .max_by_key(|o| o.age_secs())
    }

    fn any_refresh_failed(&self) -> bool {
        self.panes
            .iter()
            .any(|p| p.error.is_some() && !p.meters.is_empty())
    }
}

pub fn run(
    prog: &str,
    interval_secs: u64,
    tz: String,
    agents: Vec<&'static AgentSpec>,
    live: bool,
) -> Result<()> {
    let interval = Duration::from_secs(interval_secs);
    let (tx, rx) = mpsc::channel::<Round>();
    let (req_tx, req_rx) = mpsc::channel::<()>();

    spawn_worker(tx, req_rx, interval, tz.clone(), agents.clone(), live);

    let mut app = App {
        prog: prog.to_string(),
        tz,
        label_panes: agents.len() > 1,
        panes: agents.into_iter().map(Pane::new).collect(),
        next_fetch: None,
        interval,
    };

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &rx, &req_tx);
    ratatui::restore();
    result
}

/// 워커: 즉시 한 번 가져오고, 이후 `interval` 마다.
/// 대기 중 새로고침 요청이 오면 기다리지 않고 바로 다시 가져온다.
fn spawn_worker(
    tx: Sender<Round>,
    req_rx: Receiver<()>,
    interval: Duration,
    tz: String,
    agents: Vec<&'static AgentSpec>,
    live: bool,
) {
    let names: Vec<String> = agents.iter().map(|a| a.name.to_string()).collect();
    thread::spawn(move || {
        loop {
            let round = Round(
                multi::fetch_all(&names, &tz, live)
                    .into_iter()
                    .map(|p| match p.result {
                        Ok(snap) => PaneResult {
                            agent: p.agent,
                            outcome: Ok(snap.meters),
                            origin: Some(snap.origin),
                        },
                        Err(e) => PaneResult {
                            agent: p.agent,
                            outcome: Err(e.to_string()),
                            origin: None,
                        },
                    })
                    .collect(),
            );
            if tx.send(round).is_err() {
                return; // 메인이 끝났다
            }
            match req_rx.recv_timeout(interval) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    rx: &Receiver<Round>,
    req_tx: &Sender<()>,
) -> Result<()> {
    loop {
        while let Ok(round) = rx.try_recv() {
            apply(app, round);
            app.next_fetch = Some(Instant::now() + app.interval);
        }

        terminal.draw(|f| draw(f, app))?;

        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(());
                }
                KeyCode::Char('r') => {
                    let _ = req_tx.send(());
                }
                _ => {}
            }
        }
    }
}

fn apply(app: &mut App, round: Round) {
    let now = Local::now();
    for result in round.0 {
        let Some(pane) = app.panes.iter_mut().find(|p| p.agent.name == result.agent.name) else {
            continue;
        };
        match result.outcome {
            Ok(meters) => {
                pane.history.record(&meters, now);
                pane.meters = meters;
                pane.origin = result.origin;
                pane.error = None;
            }
            // 이전 데이터는 지우지 않는다 — 일시적 실패로 화면이 비면 더 나쁘다
            Err(msg) => pane.error = Some(msg),
        }
    }
}

fn draw(f: &mut Frame, app: &App) {
    // 오른쪽 여백을 먼저 떼어 낸다 — 머리글·구획·꼬리글이 모두 같은 폭을 쓴다
    let full = f.area();
    let canvas = Rect {
        width: full.width.saturating_sub(RIGHT_MARGIN),
        ..full
    };
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(canvas);

    draw_header(f, header, app);
    draw_panes(f, body, app);
    draw_footer(f, footer, app);
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let title = Span::styled(
        format!(" {}", app.prog),
        Style::default().add_modifier(Modifier::BOLD),
    );
    let tz = Span::styled(
        format!("  {}", app.tz),
        Style::default().fg(Color::DarkGray),
    );
    f.render_widget(Paragraph::new(Line::from(vec![title, tz])), area);
}

/// 구획을 **좌우로** 나눈다 (`A | B`). 폭은 균등하게 준다 — 어느 에이전트가
/// 더 넓어야 할 이유가 없고, 균등하면 게이지 길이가 같아 서로 비교하기 쉽다.
fn draw_panes(f: &mut Frame, area: Rect, app: &App) {
    let n = app.panes.len().max(1);
    let cols: Vec<Constraint> = (0..n).map(|_| Constraint::Ratio(1, n as u32)).collect();
    // 구획 사이 여백 — 게이지가 맞붙으면 어디까지가 한 구획인지 알 수 없다
    let slots = Layout::horizontal(cols).spacing(PANE_GAP).split(area);

    for (pane, slot) in app.panes.iter().zip(slots.iter()) {
        draw_pane(f, *slot, pane, app);
    }
}

fn chart_of(pane: &Pane, m: &Meter, area: Rect) -> Option<String> {
    let width = area.width.saturating_sub(GAUGE_INDENT) as usize;
    m.window
        .and_then(|w| pane.history.chart(&m.title, w, width))
}

fn draw_pane(f: &mut Frame, area: Rect, pane: &Pane, app: &App) {
    let mut area = area;
    if app.label_panes {
        let [label, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
            .areas(area);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", pane.agent.display),
                Style::default().add_modifier(Modifier::BOLD),
            ))),
            label,
        );
        area = rest;
    }

    if pane.meters.is_empty() {
        let msg = match &pane.error {
            Some(e) => Line::from(Span::styled(
                format!("  {e}"),
                Style::default().fg(color_for(Level::Critical)),
            )),
            None => Line::from(Span::styled(
                "  불러오는 중…",
                Style::default().fg(Color::DarkGray),
            )),
        };
        f.render_widget(Paragraph::new(msg), area);
        return;
    }

    let charts: Vec<Option<String>> = pane
        .meters
        .iter()
        .map(|m| chart_of(pane, m, area))
        .collect();
    let sizes: Vec<usize> = pane
        .meters
        .iter()
        .zip(&charts)
        .map(|(m, c)| rows_for(m, c.is_some()))
        .collect();
    let rows: Vec<Constraint> = sizes
        .iter()
        .flat_map(|n| std::iter::repeat_n(Constraint::Length(1), *n))
        .chain(std::iter::once(Constraint::Min(0)))
        .collect();
    let slots = Layout::vertical(rows).split(area);

    let mut base = 0;
    for ((m, chart), n) in pane.meters.iter().zip(&charts).zip(&sizes) {
        let delta = pane.history.delta(&m.title);
        draw_one(f, &slots[base..base + n], m, chart.as_deref(), delta.as_deref());
        base += n;
    }
}

fn draw_one(
    f: &mut Frame,
    slots: &[Rect],
    m: &Meter,
    chart: Option<&str>,
    delta: Option<&str>,
) {
    let marker = if m.emphasized { "›" } else { " " };
    let title_style = if m.emphasized {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let mut title = vec![Span::styled(format!(" {marker} {}", m.title), title_style)];
    // 앱을 켠 뒤 늘어난 양 — 차트 옆에 두면 폭이 밀려 제목 줄에 붙인다
    if let Some(delta) = delta {
        title.push(Span::styled(
            format!("  {delta}"),
            Style::default().fg(Color::Indexed(109)),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(title)), slots[0]);

    // 시간 게이지는 사용량 바로 아래 — 둘을 견줘야 페이스가 읽힌다
    let bars = std::iter::once(&m.usage).chain(m.time.as_ref());
    let mut row = 1;
    for bar in bars {
        f.render_widget(gauge(bar), indent(slots[row], GAUGE_INDENT));
        row += 1;
    }

    // 시계열 차트는 시간 게이지 바로 아래 — 가로축이 같아 세로로 맞춰 읽힌다
    if let Some(chart) = chart {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                chart,
                Style::default().fg(Color::Indexed(109)),
            ))),
            indent(slots[row], GAUGE_INDENT),
        );
        row += 1;
    }

    if let Some(note) = &m.footnote {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                note.as_str(),
                Style::default().fg(Color::DarkGray),
            ))),
            indent(slots[row], GAUGE_INDENT),
        );
    }
}

fn gauge(bar: &Bar) -> Gauge<'_> {
    Gauge::default()
        .gauge_style(Style::default().fg(color_for(bar.level)))
        .ratio(bar.fill_clamped())
        .label(bar.label.as_str())
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let mut parts = Vec::new();
    // 화면을 언제 다시 그렸는지보다 값이 언제 기준인지가 중요하다
    if let Some(origin) = app.oldest_origin() {
        parts.push(origin.text());
    }
    if let Some(secs) = app.seconds_until_refresh() {
        parts.push(format!("다음 {secs}초"));
    }
    // 데이터는 살아 있는데 갱신만 실패한 상태를 알린다
    if app.any_refresh_failed() {
        parts.push("갱신 실패".to_string());
    }
    parts.push("[r] 새로고침  [q] 종료".to_string());

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", parts.join("  ·  ")),
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

fn indent(area: Rect, by: u16) -> Rect {
    Rect {
        x: area.x.saturating_add(by).min(area.right()),
        y: area.y,
        width: area.width.saturating_sub(by),
        height: area.height,
    }
}

fn color_for(level: Level) -> Color {
    match level {
        Level::Normal => Color::Indexed(147),
        Level::Warning => Color::Indexed(179),
        Level::Critical => Color::Indexed(203),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meter::Bar;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn meter(title: &str, fill: f64, emphasized: bool) -> Meter {
        Meter {
            title: title.into(),
            usage: Bar {
                fill,
                label: format!("{:.0}% used", fill * 100.0),
                level: Level::Normal,
            },
            window: None,
            time: Some(Bar {
                fill: 0.4,
                label: "1 hour 12 minutes left".into(),
                level: Level::Normal,
            }),
            footnote: Some("Resets Aug 18 at 9:29pm (Asia/Seoul)".into()),
            emphasized,
        }
    }

    fn pane(agent: &str, meters: Vec<Meter>) -> Pane {
        Pane {
            agent: crate::registry::find(agent).unwrap(),
            meters,
            origin: None,
            history: History::default(),
            error: None,
        }
    }

    fn app_with(panes: Vec<Pane>) -> App {
        App {
            prog: "agentmeter".into(),
            tz: "Asia/Seoul".into(),
            label_panes: panes.len() > 1,
            panes,
            next_fetch: None,
            interval: Duration::from_secs(60),
        }
    }

    fn three() -> Vec<Meter> {
        vec![
            meter("Current session", 0.57, false),
            meter("Current week (all models)", 0.54, false),
            meter("Current week (Fable)", 0.74, true),
        ]
    }

    /// 버퍼를 문자열로 옮긴다.
    /// 한글처럼 2칸을 쓰는 글자는 뒤따르는 칸이 공백으로 채워져 있으므로,
    /// 글자 폭만큼 건너뛰지 않으면 "재 인 증"처럼 벌어진 문자열이 나온다.
    fn text(buf: &Buffer) -> String {
        use unicode_width::UnicodeWidthStr;
        (0..buf.area.height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0u16;
                while x < buf.area.width {
                    let sym = buf[(x, y)].symbol();
                    line.push_str(sym);
                    x += UnicodeWidthStr::width(sym).max(1) as u16;
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        text(t.backend().buffer())
    }

    #[test]
    fn renders_all_meters() {
        let out = render(&app_with(vec![pane("claude", three())]), 60, 24);
        assert!(out.contains("Current session"));
        assert!(out.contains("Current week (all models)"));
        assert!(out.contains("57% used"));
    }

    #[test]
    fn marks_the_emphasized_meter() {
        let out = render(&app_with(vec![pane("claude", three())]), 60, 24);
        let line = out
            .lines()
            .find(|l| l.contains("Current week (Fable)"))
            .unwrap();
        assert!(line.contains('›'), "강조 항목에 마커가 있어야 함");
    }

    #[test]
    fn header_shows_program_name() {
        let out = render(&app_with(vec![pane("claude", three())]), 60, 24);
        assert!(out.contains("agentmeter"));
    }

    /// 에이전트가 하나면 구획 머리글은 군더더기다.
    #[test]
    fn single_pane_has_no_label() {
        let out = render(&app_with(vec![pane("claude", three())]), 60, 24);
        assert!(!out.contains("Claude Code"), "머리글이 없어야 함:\n{out}");
    }

    /// 구획은 **좌우로** 놓인다 — 두 머리글이 같은 줄의 다른 칸에 있어야 한다.
    #[test]
    fn panes_are_side_by_side() {
        let app = app_with(vec![pane("claude", three()), pane("codex", three())]);
        let out = render(&app, 160, 24);
        let line = out
            .lines()
            .find(|l| l.contains("Claude Code"))
            .expect("머리글 줄이 있어야 함");
        assert!(
            line.contains("Codex"),
            "두 구획이 같은 줄에 나란히 있어야 함:\n{line}"
        );
        let left = line.find("Claude Code").unwrap();
        let right = line.find("Codex").unwrap();
        assert!(left < right, "설정 순서대로 왼쪽부터 놓인다");
    }

    /// 오른쪽 끝에 여백이 있어야 한다 — 게이지가 경계에 붙으면 답답하다.
    #[test]
    fn leaves_a_right_margin() {
        let app = app_with(vec![pane("claude", three())]);
        let out = render(&app, 60, 24);
        for (i, line) in out.lines().enumerate() {
            let tail: String = line.chars().rev().take(RIGHT_MARGIN as usize).collect();
            assert!(
                tail.chars().all(char::is_whitespace),
                "{i}번째 줄 오른쪽 끝이 채워져 있음: {line:?}"
            );
        }
    }

    /// 좁은 화면에서도 두 구획을 그린다 — 폭이 줄어들 뿐이다.
    #[test]
    fn narrow_screen_still_splits() {
        let app = app_with(vec![pane("claude", three()), pane("codex", three())]);
        for w in [40, 60, 80] {
            let out = render(&app, w, 24);
            assert!(out.contains("Codex"), "폭 {w} 에서도 두 번째 구획이 있어야 함");
        }
    }

    /// 화면이 짧아 다 못 그려도 패닉하지 않아야 한다.
    #[test]
    fn short_terminal_does_not_panic() {
        let app = app_with(vec![pane("claude", three()), pane("codex", three())]);
        for h in 3..=12 {
            let _ = render(&app, 80, h);
        }
    }

    /// 갱신이 실패해도 직전 데이터는 화면에 남아야 한다.
    #[test]
    fn keeps_stale_data_on_error() {
        let mut app = app_with(vec![pane("claude", three())]);
        app.panes[0].error = Some("일시적 오류".to_string());
        let out = render(&app, 60, 24);
        assert!(out.contains("Current session"), "이전 데이터가 남아야 함");
        assert!(out.contains("갱신 실패"), "실패 사실도 알려야 함");
    }

    /// 데이터가 아직 없는데 실패하면 오류를 그 구획에 보여준다.
    #[test]
    fn shows_error_when_nothing_loaded_yet() {
        let mut app = app_with(vec![pane("claude", vec![])]);
        app.panes[0].error = Some("재인증 필요".to_string());
        assert!(render(&app, 60, 24).contains("재인증 필요"));
    }

    /// 한 구획이 실패해도 다른 구획은 정상으로 그려진다.
    #[test]
    fn one_failing_pane_does_not_hide_the_other() {
        let mut app = app_with(vec![pane("claude", three()), pane("codex", vec![])]);
        app.panes[1].error = Some("조회 실패".to_string());
        let out = render(&app, 160, 24);
        assert!(out.contains("57% used"), "정상 구획은 그려져야 함:\n{out}");
        assert!(out.contains("조회 실패"), "실패 구획은 사유를 보여줘야 함");
    }

    /// 표본이 쌓이면 시계열 차트 줄이 생긴다.
    #[test]
    fn draws_the_history_chart_once_samples_exist() {
        let w = crate::meter::Window {
            resets_at: Local::now() + chrono::TimeDelta::minutes(30),
            len: chrono::TimeDelta::hours(5),
        };
        let mut meters = three();
        for m in &mut meters {
            m.window = Some(w);
        }
        let mut app = app_with(vec![pane("claude", meters.clone())]);
        assert!(!render(&app, 80, 30).contains('·'), "표본 전에는 차트가 없다");

        let now = Local::now();
        app.panes[0]
            .history
            .record(&meters, now - chrono::TimeDelta::minutes(2));
        app.panes[0].history.record(&meters, now);
        let after = render(&app, 80, 30);
        assert!(after.contains('·'), "차트의 빈 구간이 보여야 함:\n{after}");
    }
}
