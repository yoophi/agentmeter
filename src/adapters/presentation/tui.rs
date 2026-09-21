//! 상주 모드 TUI. 조회 상태는 애플리케이션 계층이, 화면 투영은 이 모듈이 맡는다.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph, Sparkline};
use ratatui::{DefaultTerminal, Frame};

use crate::application::{LiveSession, SessionState, WatchPane};
use crate::domain::usage::Severity;

use super::history;
use super::model::{self, Bar, Meter};

const TICK: Duration = Duration::from_secs(1);
const GAUGE_INDENT: u16 = 3;
const PANE_GAP: u16 = 2;
const RIGHT_MARGIN: u16 = 2;
const HISTORY_CHART_HEIGHT: usize = 3;

/// 한 구획이 내용을 자르지 않고 담으려면 필요한 폭.
///
/// 가장 긴 줄은 `  Resets Sep 16 at 1:00am (Asia/Seoul)` 같은 각주(38칸)다.
/// 이보다 좁아지면 리셋 시각의 타임존이 잘려서 화면이 깨진 것처럼 보인다.
const MIN_PANE_WIDTH: u16 = 42;

/// 구획 사이 세로 여백.
const PANE_ROW_GAP: u16 = 1;

/// 한 구획이 한도 두 개는 담아야 접는 보람이 있는 높이.
///
/// 이보다 낮아지면 가로로 좁은 것보다 세로로 잘리는 쪽이 더 나빠서, 접지 않고
/// 예전처럼 한 줄에 늘어놓는다.
const MIN_PANE_HEIGHT: u16 = 16;

fn rows_for(meter: &Meter, has_chart: bool) -> usize {
    2 + usize::from(meter.time.is_some())
        + if has_chart { HISTORY_CHART_HEIGHT } else { 0 }
        + usize::from(meter.quota_summary.is_some())
        + usize::from(meter.footnote.is_some())
        + 1
}

/// 한 프레임을 그리는 데 필요한 전부. 화면은 세션 상태를 읽기만 한다.
struct Screen<'a> {
    prog: &'a str,
    timezone: &'a str,
    /// 함께 뜬 웹 대시보드 주소. 없으면 배너를 그리지 않는다.
    web: Option<&'a str>,
    label_panes: bool,
    state: &'a SessionState,
}

/// 조회는 세션이 담당하므로 화면은 상태를 읽고 요청만 보낸다.
pub(crate) fn run(
    prog: &str,
    timezone: String,
    session: Arc<LiveSession>,
    web: Option<String>,
) -> Result<()> {
    let label_panes = session.read().watch.panes().len() > 1;
    let mut terminal = ratatui::init();
    let result = event_loop(
        &mut terminal,
        prog,
        &timezone,
        web.as_deref(),
        label_panes,
        &session,
    );
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    prog: &str,
    timezone: &str,
    web: Option<&str>,
    label_panes: bool,
    session: &LiveSession,
) -> Result<()> {
    loop {
        {
            let state = session.read();
            let screen = Screen {
                prog,
                timezone,
                web,
                label_panes,
                state: &state,
            };
            terminal.draw(|frame| draw(frame, &screen))?;
        }

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
                    if let Some(force_live) = refresh_request(code) {
                        session.request(force_live);
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

/// 헤더는 기본 1줄 + 여백이고, 웹 배너가 있으면 한 줄 더 쓴다.
fn header_height(screen: &Screen) -> u16 {
    if screen.web.is_some() { 3 } else { 2 }
}

fn draw(frame: &mut Frame, screen: &Screen) {
    let full = frame.area();
    let canvas = Rect {
        width: full.width.saturating_sub(RIGHT_MARGIN),
        ..full
    };
    let footer_height = if screen.state.watch.any_refresh_failed() {
        2
    } else {
        1
    };
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(header_height(screen)),
        Constraint::Min(0),
        Constraint::Length(footer_height),
    ])
    .areas(canvas);

    draw_header(frame, header, screen);
    draw_panes(frame, body, screen);
    draw_footer(frame, footer, screen);
}

fn draw_header(frame: &mut Frame, area: Rect, screen: &Screen) {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(" {}", screen.prog),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", screen.timezone),
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    if let Some(address) = screen.web {
        lines.push(Line::from(vec![
            Span::styled(" web server running", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("  {address}"),
                Style::default()
                    .fg(Color::Indexed(109))
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// 폭이 허락하는 만큼만 좌우로 나눈다.
///
/// 구획 수만큼 무조건 쪼개면 provider 가 늘어날수록 한 칸이 좁아져 각주가 잘린다.
/// 열 수를 폭으로 정하고, 남는 구획은 아래 행으로 넘긴다. 행이 여러 개가 되면
/// 열 수를 다시 고르게 나눠 마지막 행만 휑하게 비지 않도록 한다.
fn pane_grid(count: usize, width: u16, height: u16) -> (usize, usize) {
    let count = count.max(1);
    let fits = ((width + PANE_GAP) / (MIN_PANE_WIDTH + PANE_GAP)).max(1) as usize;
    let columns = count.min(fits);
    let rows = count.div_ceil(columns);

    // 접었을 때 각 행이 너무 낮으면 가로 잘림보다 세로 잘림이 더 크다.
    let row_height = (height + PANE_ROW_GAP) / rows.max(1) as u16;
    if rows > 1 && row_height < MIN_PANE_HEIGHT + PANE_ROW_GAP {
        return (count, 1);
    }
    (count.div_ceil(rows), rows)
}

fn draw_panes(frame: &mut Frame, area: Rect, screen: &Screen) {
    let panes = screen.state.watch.panes();
    let (columns, rows) = pane_grid(panes.len(), area.width, area.height);

    let row_slots = Layout::vertical((0..rows).map(|_| Constraint::Ratio(1, rows as u32)))
        .spacing(PANE_ROW_GAP)
        .split(area);

    for (row, row_area) in row_slots.iter().enumerate() {
        let slots = Layout::horizontal((0..columns).map(|_| Constraint::Ratio(1, columns as u32)))
            .spacing(PANE_GAP)
            .split(*row_area);
        for (column, slot) in slots.iter().enumerate() {
            let Some(pane) = panes.get(row * columns + column) else {
                break;
            };
            draw_pane(frame, *slot, pane, screen);
        }
    }
}

fn chart_of(pane: &WatchPane, meter: &Meter, area: Rect) -> Option<Vec<Option<u64>>> {
    let width = area.width.saturating_sub(GAUGE_INDENT) as usize;
    meter
        .window
        .and_then(|window| history::chart(pane.samples(&meter.id, Some(window)), window, width))
}

fn draw_pane(frame: &mut Frame, area: Rect, pane: &WatchPane, screen: &Screen) {
    let mut area = area;
    if screen.label_panes {
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
        .map(|snapshot| model::project(snapshot, screen.timezone, Local::now()))
        .unwrap_or_default();

    if meters.is_empty() {
        let message = match &pane.error {
            Some(error) => Line::from(Span::styled(
                format!("  {error}"),
                Style::default().fg(color_for(Severity::Critical)),
            )),
            None => Line::from(Span::styled(
                if pane.snapshot.is_some() {
                    "  No usage windows to show"
                } else {
                    "  Loading..."
                },
                Style::default().fg(Color::DarkGray),
            )),
        };
        let [message_area, credits_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        frame.render_widget(Paragraph::new(message), message_area);
        draw_reset_credits(frame, credits_area, pane, screen.timezone);
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
    draw_reset_credits(frame, slots[base], pane, screen.timezone);
}

fn draw_reset_credits(frame: &mut Frame, area: Rect, pane: &WatchPane, timezone: &str) {
    let Some(credits) = pane
        .snapshot
        .as_ref()
        .and_then(|snapshot| model::reset_credits(snapshot, timezone))
    else {
        return;
    };
    let mut lines = vec![Line::default(), Line::from(format!("   {}", credits.label))];
    if let Some(expiry) = credits.expiry_label {
        lines.push(Line::from(format!("   {expiry}")));
    }
    frame.render_widget(Paragraph::new(lines), area);
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

    if let Some(summary) = &meter.quota_summary {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                summary.as_str(),
                Style::default().fg(Color::DarkGray),
            ))),
            indent(slots[row], GAUGE_INDENT),
        );
        row += 1;
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

fn draw_footer(frame: &mut Frame, area: Rect, screen: &Screen) {
    let mut parts = Vec::new();
    let now = Local::now();
    let panes = screen.state.watch.panes();
    if let Some(origin) = screen.state.watch.oldest_origin(now) {
        parts.push(model::origin_text(origin, now));
    }
    if let Some(seconds) = screen.state.seconds_until_refresh(now) {
        parts.push(format!("next in {seconds}s"));
    }
    let controls = if panes
        .iter()
        .any(|pane| matches!(pane.agent.name, "claude" | "kiro"))
    {
        "[r] refresh  [R] live fetch  [q] quit"
    } else {
        "[r] refresh  [q] quit"
    };
    parts.push(controls.to_string());

    let mut lines = vec![Line::from(Span::styled(
        format!(" {}", parts.join("  ·  ")),
        Style::default().fg(Color::DarkGray),
    ))];
    let errors: Vec<String> = panes
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
            format!(" refresh failed: {}", errors.join(" · ")),
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
    use crate::application::{AgentInfo, AgentResult, FetchError, WatchState};
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

    fn state_of(watch: WatchState) -> SessionState {
        SessionState {
            watch,
            refreshing: false,
            next_refresh_at: None,
        }
    }

    fn state_with(names: &[&'static str], with_data: bool) -> SessionState {
        let infos: Vec<_> = names.iter().map(|name| info(name)).collect();
        let mut watch = WatchState::new(infos.clone());
        if with_data {
            let results = infos
                .iter()
                .map(|agent| AgentResult {
                    agent: *agent,
                    result: Ok(UsageSnapshot::live(limits(), Local::now())),
                })
                .collect();
            watch.apply(results);
        }
        state_of(watch)
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

    fn render_web(state: &SessionState, web: Option<&str>, width: u16, height: u16) -> String {
        let screen = Screen {
            prog: "agentmeter",
            timezone: "Asia/Seoul",
            web,
            label_panes: state.watch.panes().len() > 1,
            state,
        };
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, &screen)).unwrap();
        text(terminal.backend().buffer())
    }

    fn render(state: &SessionState, width: u16, height: u16) -> String {
        render_web(state, None, width, height)
    }

    #[test]
    fn renders_projected_domain_limits() {
        let output = render(&state_with(&["claude"], true), 60, 24);
        assert!(output.contains("Current session"));
        assert!(output.contains("Current week (Fable)"));
        assert!(output.contains("57% used"));
        assert!(!output.contains("Claude Code"));
    }

    #[test]
    fn multiple_panes_are_side_by_side() {
        let output = render(&state_with(&["claude", "codex"], true), 160, 24);
        let line = output
            .lines()
            .find(|line| line.contains("Claude Code"))
            .unwrap();
        assert!(line.contains("Codex"), "{line}");
    }

    #[test]
    fn error_is_visible_without_data() {
        let mut state = state_with(&["claude"], false);
        state.watch.apply(vec![AgentResult {
            agent: info("claude"),
            result: Err(FetchError::Other(anyhow::anyhow!(
                "re-authentication required"
            ))),
        }]);
        assert!(render(&state, 60, 24).contains("re-authentication required"));
    }

    #[test]
    fn stale_data_and_refresh_failure_are_both_visible() {
        let mut state = state_with(&["claude"], true);
        state.watch.apply(vec![AgentResult {
            agent: info("claude"),
            result: Err(FetchError::Other(anyhow::anyhow!("HTTP 429"))),
        }]);
        let output = render(&state, 60, 24);
        assert!(output.contains("Current session"));
        assert!(output.contains("refresh failed"));
        assert!(output.contains("HTTP 429"));
    }

    #[test]
    fn short_and_narrow_terminals_do_not_panic() {
        let state = state_with(&["claude", "codex"], true);
        for width in [40, 60, 80] {
            for height in 3..=12 {
                let _ = render(&state, width, height);
                let _ = render_web(&state, Some("http://127.0.0.1:8080"), width, height);
            }
        }
    }

    #[test]
    fn history_is_rendered_after_two_application_updates() {
        let infos = vec![info("claude")];
        let mut watch = WatchState::new(infos.clone());
        for offset in [2, 1] {
            let at = Local::now() - TimeDelta::minutes(offset);
            watch.apply(vec![AgentResult {
                agent: infos[0],
                result: Ok(UsageSnapshot::live(limits(), at)),
            }]);
        }
        assert!(render(&state_of(watch), 80, 30).contains('·'));
    }

    /// 폭이 모자라면 열을 줄이고 행을 늘린다. 152칸에 4구획이 이 화면의 사례다.
    #[test]
    fn panes_wrap_instead_of_getting_too_narrow() {
        // 152x71 터미널의 본문 크기(오른쪽 여백과 머리말·꼬리말 제외).
        assert_eq!(pane_grid(4, 150, 68), (2, 2), "4구획은 2x2 로 접혀야 함");
        assert_eq!(
            pane_grid(3, 150, 68),
            (3, 1),
            "3구획은 48칸씩이라 한 줄에 들어감"
        );
        assert_eq!(pane_grid(2, 150, 68), (2, 1));
        assert_eq!(pane_grid(1, 150, 68), (1, 1));
        // 아주 좁으면 한 열로 쌓는다.
        assert_eq!(pane_grid(4, 60, 68), (1, 4));
        // 넓으면 그대로 나란히.
        assert_eq!(pane_grid(4, 400, 68), (4, 1));
    }

    /// 세로가 모자라면 접지 않는다. 가로로 좁은 것보다 세로로 잘리는 쪽이 더 나쁘다.
    #[test]
    fn a_short_terminal_keeps_panes_on_one_row() {
        assert_eq!(pane_grid(4, 150, 20), (4, 1), "낮은 화면은 예전처럼 한 줄");
        assert_eq!(pane_grid(4, 60, 20), (4, 1));
        // 높이가 충분해지면 다시 접는다.
        assert_eq!(pane_grid(4, 150, 34), (2, 2));
    }

    /// 접힌 뒤에는 각주가 잘리지 않아야 한다.
    #[test]
    fn a_folded_pane_keeps_the_reset_footnote_whole() {
        let state = state_with(&["claude", "codex", "glm", "kiro"], true);
        let screen = Screen {
            prog: "agentmeter",
            timezone: "Asia/Seoul",
            web: None,
            label_panes: true,
            state: &state,
        };
        let mut terminal = Terminal::new(TestBackend::new(152, 71)).unwrap();
        terminal.draw(|frame| draw(frame, &screen)).unwrap();
        let output = text(terminal.backend().buffer());
        assert!(
            output.contains("(Asia/Seoul)"),
            "타임존이 잘리지 않아야 함:\n{output}"
        );
    }

    #[test]
    fn refresh_keys_choose_cached_or_fresh_policy() {
        assert_eq!(refresh_request(KeyCode::Char('r')), Some(false));
        assert_eq!(refresh_request(KeyCode::Char('R')), Some(true));
    }

    #[test]
    fn cached_providers_advertise_direct_refresh() {
        assert!(render(&state_with(&["claude"], true), 100, 30).contains("[R] live fetch"));
        assert!(render(&state_with(&["kiro"], true), 100, 30).contains("[R] live fetch"));
    }

    #[test]
    fn the_web_banner_shows_that_the_server_runs_and_where() {
        let state = state_with(&["claude"], true);
        let output = render_web(&state, Some("http://127.0.0.1:54321"), 80, 30);
        let banner = output
            .lines()
            .find(|line| line.contains("web server running"))
            .expect("웹 배너가 상단에 있어야 함");
        assert!(banner.contains("http://127.0.0.1:54321"), "{banner}");
        let rows: Vec<&str> = output.lines().collect();
        assert!(rows[0].contains("agentmeter"), "{:?}", rows[0]);
        assert_eq!(rows[1], banner, "배너는 헤더 둘째 줄이어야 함");
    }

    #[test]
    fn without_a_server_the_header_keeps_its_original_height() {
        let state = state_with(&["claude"], true);
        let plain = render(&state, 80, 30);
        assert!(!plain.contains("web server"));
        let first_meter = plain
            .lines()
            .position(|line| line.contains("Current session"))
            .unwrap();
        let with_banner = render_web(&state, Some("http://127.0.0.1:1"), 80, 30);
        let shifted = with_banner
            .lines()
            .position(|line| line.contains("Current session"))
            .unwrap();
        assert_eq!(shifted, first_meter + 1, "배너는 본문을 한 줄만 밀어야 함");
    }

    #[test]
    fn the_footer_counts_down_to_the_next_session_refresh() {
        let mut state = state_with(&["claude"], true);
        // 남은 시간은 초 단위로 절삭되므로 경계에 걸리지 않게 여유를 준다.
        state.next_refresh_at =
            Some(Local::now() + TimeDelta::seconds(42) + TimeDelta::milliseconds(900));
        let output = render(&state, 100, 30);
        assert!(output.contains("next in 42s"), "{output}");
    }
    #[test]
    fn credits_survive_refresh_failure_and_render_without_windows() {
        let mut state = state_with(&["codex"], false);
        let mut snapshot = UsageSnapshot::live(vec![], Local::now());
        snapshot.reset_credits = Some(crate::domain::usage::ResetCredits {
            available_count: 2,
            earliest_known_expires_at: Some(Local::now() + TimeDelta::days(1)),
        });
        state.watch.apply(vec![AgentResult {
            agent: info("codex"),
            result: Ok(snapshot),
        }]);
        state.watch.apply(vec![AgentResult {
            agent: info("codex"),
            result: Err(FetchError::Other(anyhow::anyhow!("offline"))),
        }]);
        let output = render(&state, 100, 24);
        assert!(output.contains("Reset credits: 2"), "{output}");
        assert!(output.contains("Known expiry:"), "{output}");
        assert!(output.contains("refresh failed"));
        assert!(output.contains("offline"));
        for width in [10, 30, 60] {
            for height in [3, 5, 12] {
                let _ = render(&state, width, height);
            }
        }
    }
    #[test]
    fn reset_credits_follow_all_meters_with_top_spacing() {
        let mut state = state_with(&["codex"], false);
        let mut snapshot = UsageSnapshot::live(limits(), Local::now());
        snapshot.reset_credits = Some(crate::domain::usage::ResetCredits {
            available_count: 0,
            earliest_known_expires_at: None,
        });
        state.watch.apply(vec![AgentResult {
            agent: info("codex"),
            result: Ok(snapshot),
        }]);
        let output = render(&state, 100, 50);
        let rows: Vec<_> = output.lines().collect();
        let credits = rows
            .iter()
            .position(|line| line.contains("Reset credits: 0"))
            .unwrap();
        let last_reset = rows
            .iter()
            .rposition(|line| line.contains("Resets "))
            .unwrap();
        assert!(credits > last_reset + 1, "{output}");
        assert!(rows[credits - 1].trim().is_empty());
        assert!(!output.contains("Known expiry:"));
        state.watch.apply(vec![AgentResult {
            agent: info("codex"),
            result: Ok(UsageSnapshot::live(limits(), Local::now())),
        }]);
        assert!(!render(&state, 100, 50).contains("Reset credits:"));
    }
}
