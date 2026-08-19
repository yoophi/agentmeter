//! 상주 모드 TUI. 조회 상태는 애플리케이션 계층이, 화면 투영은 이 모듈이 맡는다.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph, Sparkline};
use ratatui::{DefaultTerminal, Frame};

use crate::application::{
    AgentResult, FetchPolicy, HistoryRepository, RefreshCoordinator, RefreshDecision,
    UsageApplication, WatchPane, WatchState,
};
use crate::domain::usage::Severity;

use super::history;
use super::model::{self, Bar, Meter};

const TICK: Duration = Duration::from_secs(1);
const GAUGE_INDENT: u16 = 3;
const PANE_GAP: u16 = 2;
const RIGHT_MARGIN: u16 = 2;
const HISTORY_CHART_HEIGHT: usize = 3;

fn rows_for(meter: &Meter, has_chart: bool) -> usize {
    2 + usize::from(meter.time.is_some())
        + if has_chart { HISTORY_CHART_HEIGHT } else { 0 }
        + usize::from(meter.footnote.is_some())
        + 1
}

struct Round(Vec<AgentResult>);

struct App {
    prog: String,
    timezone: String,
    state: WatchState,
    next_fetch: Option<Instant>,
    interval: Duration,
    label_panes: bool,
}

impl App {
    fn seconds_until_refresh(&self) -> Option<u64> {
        Some(
            self.next_fetch?
                .saturating_duration_since(Instant::now())
                .as_secs(),
        )
    }
}

pub(crate) fn run(
    prog: &str,
    interval_secs: u64,
    timezone: String,
    application: UsageApplication,
    history: Arc<dyn HistoryRepository>,
    names: Vec<String>,
    live: bool,
) -> Result<()> {
    let interval = Duration::from_secs(interval_secs);
    let (tx, rx) = mpsc::channel::<Round>();
    let (request_tx, request_rx) = mpsc::channel::<FetchPolicy>();
    let agents = application.info(&names)?;
    let refresh = Arc::new(RefreshCoordinator::new(live));
    let initial = match refresh.request(false) {
        RefreshDecision::Execute(policy) => policy,
        RefreshDecision::Queued => unreachable!("idle coordinator queues initial refresh"),
    };

    spawn_worker(
        tx,
        request_rx,
        interval,
        application,
        names,
        Arc::clone(&refresh),
        initial,
    );

    let mut app = App {
        prog: prog.to_string(),
        timezone,
        label_panes: agents.len() > 1,
        state: WatchState::persistent(agents, history),
        next_fetch: None,
        interval,
    };

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &rx, &request_tx, &refresh);
    ratatui::restore();
    result
}

fn spawn_worker(
    tx: Sender<Round>,
    request_rx: Receiver<FetchPolicy>,
    interval: Duration,
    application: UsageApplication,
    names: Vec<String>,
    refresh: Arc<RefreshCoordinator>,
    initial: FetchPolicy,
) {
    thread::spawn(move || {
        let mut policy = initial;
        loop {
            let results = application.query(&names, policy).unwrap_or_default();
            if tx.send(Round(results)).is_err() {
                return;
            }
            if let Some(pending) = refresh.complete() {
                policy = pending;
                continue;
            }
            match request_rx.recv_timeout(interval) {
                Ok(requested) => policy = requested,
                Err(RecvTimeoutError::Timeout) => {
                    policy = match refresh.request(false) {
                        RefreshDecision::Execute(policy) => policy,
                        RefreshDecision::Queued => continue,
                    };
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    rx: &Receiver<Round>,
    request_tx: &Sender<FetchPolicy>,
    refresh: &RefreshCoordinator,
) -> Result<()> {
    loop {
        while let Ok(round) = rx.try_recv() {
            app.state.apply(round.0);
            app.next_fetch = Some(Instant::now() + app.interval);
        }

        terminal.draw(|frame| draw(frame, app))?;

        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(());
                }
                code => {
                    if let Some(force_live) = refresh_request(code)
                        && let RefreshDecision::Execute(policy) = refresh.request(force_live)
                    {
                        let _ = request_tx.send(policy);
                    }
                }
            }
        }
    }
}

fn refresh_request(code: KeyCode) -> Option<bool> {
    match code {
        KeyCode::Char('r') => Some(false),
        KeyCode::Char('R') => Some(true),
        _ => None,
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let full = frame.area();
    let canvas = Rect {
        width: full.width.saturating_sub(RIGHT_MARGIN),
        ..full
    };
    let footer_height = if app.state.any_refresh_failed() { 2 } else { 1 };
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(footer_height),
    ])
    .areas(canvas);

    draw_header(frame, header, app);
    draw_panes(frame, body, app);
    draw_footer(frame, footer, app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {}", app.prog),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}", app.timezone),
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        area,
    );
}

fn draw_panes(frame: &mut Frame, area: Rect, app: &App) {
    let count = app.state.panes().len().max(1);
    let columns: Vec<Constraint> = (0..count)
        .map(|_| Constraint::Ratio(1, count as u32))
        .collect();
    let slots = Layout::horizontal(columns).spacing(PANE_GAP).split(area);

    for (pane, slot) in app.state.panes().iter().zip(slots.iter()) {
        draw_pane(frame, *slot, pane, app);
    }
}

fn chart_of(pane: &WatchPane, meter: &Meter, area: Rect) -> Option<Vec<Option<u64>>> {
    let width = area.width.saturating_sub(GAUGE_INDENT) as usize;
    meter
        .window
        .and_then(|window| history::chart(pane.samples(&meter.id, Some(window)), window, width))
}

fn draw_pane(frame: &mut Frame, area: Rect, pane: &WatchPane, app: &App) {
    let mut area = area;
    if app.label_panes {
        let [label, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", pane.agent.display),
                Style::default().add_modifier(Modifier::BOLD),
            ))),
            label,
        );
        area = rest;
    }

    let meters = pane
        .snapshot
        .as_ref()
        .map(|snapshot| model::project(snapshot, &app.timezone, Local::now()))
        .unwrap_or_default();

    if meters.is_empty() {
        let message = match &pane.error {
            Some(error) => Line::from(Span::styled(
                format!("  {error}"),
                Style::default().fg(color_for(Severity::Critical)),
            )),
            None => Line::from(Span::styled(
                "  불러오는 중…",
                Style::default().fg(Color::DarkGray),
            )),
        };
        frame.render_widget(Paragraph::new(message), area);
        return;
    }

    let charts: Vec<Option<Vec<Option<u64>>>> = meters
        .iter()
        .map(|meter| chart_of(pane, meter, area))
        .collect();
    let sizes: Vec<usize> = meters
        .iter()
        .zip(&charts)
        .map(|(meter, chart)| rows_for(meter, chart.is_some()))
        .collect();
    let rows: Vec<Constraint> = sizes
        .iter()
        .flat_map(|size| std::iter::repeat_n(Constraint::Length(1), *size))
        .chain(std::iter::once(Constraint::Min(0)))
        .collect();
    let slots = Layout::vertical(rows).split(area);

    let mut base = 0;
    for ((meter, chart), size) in meters.iter().zip(&charts).zip(&sizes) {
        let delta = history::delta(pane.samples(&meter.id, meter.window));
        draw_one(
            frame,
            &slots[base..base + size],
            meter,
            chart.as_deref(),
            delta.as_deref(),
        );
        base += size;
    }
}

fn draw_one(
    frame: &mut Frame,
    slots: &[Rect],
    meter: &Meter,
    chart: Option<&[Option<u64>]>,
    delta: Option<&str>,
) {
    let marker = if meter.emphasized { "›" } else { " " };
    let title_style = if meter.emphasized {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let mut title = vec![Span::styled(
        format!(" {marker} {}", meter.title),
        title_style,
    )];
    if let Some(delta) = delta {
        title.push(Span::styled(
            format!("  {delta}"),
            Style::default().fg(Color::Indexed(109)),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(title)), slots[0]);

    let bars = std::iter::once(&meter.usage).chain(meter.time.as_ref());
    let mut row = 1;
    for bar in bars {
        frame.render_widget(gauge(bar), indent(slots[row], GAUGE_INDENT));
        row += 1;
    }

    if let Some(chart) = chart {
        let first = slots[row];
        let last = slots[row + HISTORY_CHART_HEIGHT - 1];
        let chart_area = Rect {
            x: first.x,
            y: first.y,
            width: first.width,
            height: last.bottom().saturating_sub(first.y),
        };
        frame.render_widget(
            Sparkline::default()
                .data(chart.iter().copied())
                .max(100)
                .style(Style::default().fg(Color::Indexed(109)))
                .absent_value_style(Style::default().fg(Color::DarkGray))
                .absent_value_symbol("·"),
            indent(chart_area, GAUGE_INDENT),
        );
        row += HISTORY_CHART_HEIGHT;
    }

    if let Some(note) = &meter.footnote {
        frame.render_widget(
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

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let mut parts = Vec::new();
    let now = Local::now();
    if let Some(origin) = app.state.oldest_origin(now) {
        parts.push(model::origin_text(origin, now));
    }
    if let Some(seconds) = app.seconds_until_refresh() {
        parts.push(format!("다음 {seconds}초"));
    }
    let controls = if app
        .state
        .panes()
        .iter()
        .any(|pane| pane.agent.name == "claude")
    {
        "[r] 새로고침  [R] HTTP 조회  [q] 종료"
    } else {
        "[r] 새로고침  [q] 종료"
    };
    parts.push(controls.to_string());

    let mut lines = vec![Line::from(Span::styled(
        format!(" {}", parts.join("  ·  ")),
        Style::default().fg(Color::DarkGray),
    ))];
    let errors: Vec<String> = app
        .state
        .panes()
        .iter()
        .filter_map(|pane| {
            pane.snapshot.as_ref()?;
            pane.error
                .as_ref()
                .map(|error| format!("{}: {error}", pane.agent.display))
        })
        .collect();
    if !errors.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" 갱신 실패: {}", errors.join(" · ")),
            Style::default().fg(color_for(Severity::Critical)),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn indent(area: Rect, by: u16) -> Rect {
    Rect {
        x: area.x.saturating_add(by).min(area.right()),
        y: area.y,
        width: area.width.saturating_sub(by),
        height: area.height,
    }
}

fn color_for(severity: Severity) -> Color {
    match severity {
        Severity::Normal => Color::Indexed(147),
        Severity::Warning => Color::Indexed(179),
        Severity::Critical => Color::Indexed(203),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::application::{AgentInfo, FetchError};
    use crate::domain::usage::{UsageLimit, UsageSnapshot};

    fn info(name: &'static str) -> AgentInfo {
        AgentInfo {
            name,
            display: match name {
                "claude" => "Claude Code",
                "codex" => "Codex",
                _ => name,
            },
        }
    }

    fn limits() -> Vec<UsageLimit> {
        let reset = Local::now() + TimeDelta::hours(1);
        vec![
            UsageLimit::new(
                "session:all",
                None,
                57.0,
                None,
                false,
                Some(TimeDelta::hours(5)),
                Some(reset),
            ),
            UsageLimit::new(
                "weekly:all",
                None,
                54.0,
                None,
                false,
                Some(TimeDelta::days(7)),
                Some(reset),
            ),
            UsageLimit::new(
                "weekly:fable",
                Some("Fable".into()),
                74.0,
                None,
                true,
                Some(TimeDelta::days(7)),
                Some(reset),
            ),
        ]
    }

    fn app_with(names: &[&'static str], with_data: bool) -> App {
        let infos: Vec<_> = names.iter().map(|name| info(name)).collect();
        let mut state = WatchState::new(infos.clone());
        if with_data {
            let results = infos
                .iter()
                .map(|agent| AgentResult {
                    agent: *agent,
                    result: Ok(UsageSnapshot::live(limits(), Local::now())),
                })
                .collect();
            state.apply(results);
        }
        App {
            prog: "agentmeter".into(),
            timezone: "Asia/Seoul".into(),
            label_panes: names.len() > 1,
            state,
            next_fetch: None,
            interval: Duration::from_secs(60),
        }
    }

    fn text(buffer: &Buffer) -> String {
        use unicode_width::UnicodeWidthStr;
        (0..buffer.area.height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0u16;
                while x < buffer.area.width {
                    let symbol = buffer[(x, y)].symbol();
                    line.push_str(symbol);
                    x += UnicodeWidthStr::width(symbol).max(1) as u16;
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        text(terminal.backend().buffer())
    }

    #[test]
    fn renders_projected_domain_limits() {
        let output = render(&app_with(&["claude"], true), 60, 24);
        assert!(output.contains("Current session"));
        assert!(output.contains("Current week (Fable)"));
        assert!(output.contains("57% used"));
        assert!(!output.contains("Claude Code"));
    }

    #[test]
    fn multiple_panes_are_side_by_side() {
        let output = render(&app_with(&["claude", "codex"], true), 160, 24);
        let line = output
            .lines()
            .find(|line| line.contains("Claude Code"))
            .unwrap();
        assert!(line.contains("Codex"), "{line}");
    }

    #[test]
    fn error_is_visible_without_data() {
        let mut app = app_with(&["claude"], false);
        app.state.apply(vec![AgentResult {
            agent: info("claude"),
            result: Err(FetchError::Other(anyhow::anyhow!("재인증 필요"))),
        }]);
        assert!(render(&app, 60, 24).contains("재인증 필요"));
    }

    #[test]
    fn stale_data_and_refresh_failure_are_both_visible() {
        let mut app = app_with(&["claude"], true);
        app.state.apply(vec![AgentResult {
            agent: info("claude"),
            result: Err(FetchError::Other(anyhow::anyhow!("HTTP 429"))),
        }]);
        let output = render(&app, 60, 24);
        assert!(output.contains("Current session"));
        assert!(output.contains("갱신 실패"));
        assert!(output.contains("HTTP 429"));
    }

    #[test]
    fn short_and_narrow_terminals_do_not_panic() {
        let app = app_with(&["claude", "codex"], true);
        for width in [40, 60, 80] {
            for height in 3..=12 {
                let _ = render(&app, width, height);
            }
        }
    }

    #[test]
    fn history_is_rendered_after_two_application_updates() {
        let infos = vec![info("claude")];
        let mut state = WatchState::new(infos.clone());
        for offset in [2, 1] {
            let at = Local::now() - TimeDelta::minutes(offset);
            state.apply(vec![AgentResult {
                agent: infos[0],
                result: Ok(UsageSnapshot::live(limits(), at)),
            }]);
        }
        let app = App {
            prog: "agentmeter".into(),
            timezone: "Asia/Seoul".into(),
            label_panes: false,
            state,
            next_fetch: None,
            interval: Duration::from_secs(60),
        };
        assert!(render(&app, 80, 30).contains('·'));
    }

    #[test]
    fn refresh_keys_choose_cached_or_fresh_policy() {
        assert_eq!(refresh_request(KeyCode::Char('r')), Some(false));
        assert_eq!(refresh_request(KeyCode::Char('R')), Some(true));
    }

    #[test]
    fn claude_footer_advertises_http_refresh() {
        assert!(render(&app_with(&["claude"], true), 100, 30).contains("[R] HTTP 조회"));
    }
}
