//! 상주 모드 TUI.
//!
//! 네트워크 호출은 워커 스레드가 담당한다. 메인 스레드가 직접 fetch 하면
//! 타임아웃(최대 15초) 동안 화면이 얼어붙고 키 입력도 안 먹는다.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::meter::{Level, Meter, Origin, Snapshot};

/// 유휴 시 화면을 다시 그리는 주기. 가장 잘게 변하는 것이 `다음 N초` 라 1초면 충분하다.
/// 키 입력은 `event::poll` 이 즉시 깨우므로 이 값과 무관하게 바로 반응한다.
const TICK: Duration = Duration::from_secs(1);

/// 제목 / 사용량 / 시간 / 각주 / 여백
const ROWS_PER_METER: usize = 5;

use crate::app::Fetch;

enum Msg {
    Data(Snapshot),
    Failed(String),
}

struct App {
    prog: String,
    meters: Vec<Meter>,
    /// 값을 언제 어디서 가져왔는지 (캐시일 수 있다)
    origin: Option<Origin>,
    error: Option<String>,
    next_fetch: Option<Instant>,
    interval: Duration,
    tz: String,
}

impl App {
    fn seconds_until_refresh(&self) -> Option<u64> {
        let next = self.next_fetch?;
        Some(next.saturating_duration_since(Instant::now()).as_secs())
    }
}

pub fn run(prog: &str, interval_secs: u64, tz: String, fetch: Fetch) -> Result<()> {
    let interval = Duration::from_secs(interval_secs);
    let (tx, rx) = mpsc::channel::<Msg>();
    let (req_tx, req_rx) = mpsc::channel::<()>();

    spawn_worker(tx, req_rx, interval, fetch);

    let mut app = App {
        prog: prog.to_string(),
        meters: Vec::new(),
        origin: None,
        error: None,
        next_fetch: None,
        interval,
        tz,
    };

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &rx, &req_tx);
    ratatui::restore();
    result
}

/// 워커: 즉시 한 번 가져오고, 이후 `interval` 마다.
/// 대기 중 새로고침 요청이 오면 기다리지 않고 바로 다시 가져온다.
fn spawn_worker(tx: Sender<Msg>, req_rx: Receiver<()>, interval: Duration, fetch: Fetch) {
    thread::spawn(move || {
        loop {
            let msg = match fetch() {
                Ok(snap) => Msg::Data(snap),
                Err(e) => Msg::Failed(e.to_string()),
            };
            if tx.send(msg).is_err() {
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
    rx: &Receiver<Msg>,
    req_tx: &Sender<()>,
) -> Result<()> {
    loop {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Data(snap) => {
                    app.meters = snap.meters;
                    app.origin = Some(snap.origin);
                    app.error = None;
                }
                // 이전 데이터는 지우지 않는다 — 일시적 실패로 화면이 비면 더 나쁘다
                Msg::Failed(m) => app.error = Some(m),
            }
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

fn draw(f: &mut Frame, app: &App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(f.area());

    draw_header(f, header, app);
    draw_meters(f, body, app);
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

fn draw_meters(f: &mut Frame, area: Rect, app: &App) {
    if app.meters.is_empty() {
        let msg = match &app.error {
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

    // 항목당 제목 / 사용량 / 시간 / 각주 / 여백
    let rows: Vec<Constraint> = app
        .meters
        .iter()
        .flat_map(|_| [Constraint::Length(1); ROWS_PER_METER])
        .chain(std::iter::once(Constraint::Min(0)))
        .collect();
    let slots = Layout::vertical(rows).split(area);

    // `Layout::split` 은 화면이 작아도 제약 수만큼 Rect 를 돌려주므로
    // (넘치는 것은 높이 0) 항목마다 정확히 ROWS_PER_METER 칸이 있다.
    for (rows, m) in slots.chunks_exact(ROWS_PER_METER).zip(&app.meters) {
        draw_one(f, rows, m);
    }
}

fn draw_one(f: &mut Frame, slots: &[Rect], m: &Meter) {
    let marker = if m.emphasized { "›" } else { " " };
    let title_style = if m.emphasized {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {marker} {}", m.title),
            title_style,
        ))),
        slots[0],
    );

    // 시간 게이지는 사용량 바로 아래 — 둘을 견줘야 페이스가 읽힌다
    let bars = std::iter::once(&m.usage).chain(m.time.as_ref());
    let mut row = 1;
    for bar in bars {
        f.render_widget(
            Gauge::default()
                .gauge_style(Style::default().fg(color_for(bar.level)))
                .ratio(bar.fill_clamped())
                .label(bar.label.as_str()),
            indent(slots[row], 3),
        );
        row += 1;
    }

    if let Some(note) = &m.footnote {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                note.as_str(),
                Style::default().fg(Color::DarkGray),
            ))),
            indent(slots[row], 3),
        );
    }
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let mut parts = Vec::new();
    // 화면을 언제 다시 그렸는지보다 값이 언제 기준인지가 중요하다
    if let Some(origin) = app.origin {
        parts.push(origin.text());
    }
    if let Some(secs) = app.seconds_until_refresh() {
        parts.push(format!("다음 {secs}초"));
    }
    // 데이터는 살아 있는데 갱신만 실패한 상태를 알린다
    if app.error.is_some() && !app.meters.is_empty() {
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
            time: Some(Bar {
                fill: 0.4,
                label: "1 hour 12 minutes left".into(),
                level: Level::Normal,
            }),
            footnote: Some("Resets Aug 18 at 9:29pm (Asia/Seoul)".into()),
            emphasized,
        }
    }

    fn app_with(meters: Vec<Meter>) -> App {
        App {
            prog: "ccmeter".into(),
            meters,
            origin: None,
            error: None,
            next_fetch: None,
            interval: Duration::from_secs(60),
            tz: "Asia/Seoul".to_string(),
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
        let out = render(&app_with(three()), 60, 20);
        assert!(out.contains("Current session"));
        assert!(out.contains("Current week (all models)"));
        assert!(out.contains("Current week (Fable)"));
        assert!(out.contains("57% used"));
        assert!(out.contains("74% used"));
    }

    #[test]
    fn marks_the_emphasized_meter() {
        let out = render(&app_with(three()), 60, 20);
        let line = out
            .lines()
            .find(|l| l.contains("Current week (Fable)"))
            .unwrap();
        assert!(line.contains('›'), "강조 항목에 마커가 있어야 함");
    }

    /// 프로그램 이름은 헤더에 그대로 나와야 한다 — 두 도구가 같은 코드를 쓴다.
    #[test]
    fn header_shows_program_name() {
        let mut app = app_with(three());
        app.prog = "codexmeter".into();
        assert!(render(&app, 60, 20).contains("codexmeter"));
    }

    /// 화면이 짧아 다 못 그려도 패닉하지 않아야 한다.
    #[test]
    fn short_terminal_does_not_panic() {
        for h in 3..=12 {
            let _ = render(&app_with(three()), 40, h);
        }
    }

    /// 갱신이 실패해도 직전 데이터는 화면에 남아야 한다.
    #[test]
    fn keeps_stale_data_on_error() {
        let mut app = app_with(three());
        app.error = Some("일시적 오류".to_string());
        let out = render(&app, 60, 20);
        assert!(out.contains("Current session"), "이전 데이터가 남아야 함");
        assert!(out.contains("갱신 실패"), "실패 사실도 알려야 함");
    }

    /// 데이터가 아직 없는데 실패하면 오류를 본문에 보여준다.
    #[test]
    fn shows_error_when_nothing_loaded_yet() {
        let mut app = app_with(vec![]);
        app.error = Some("재인증 필요".to_string());
        assert!(render(&app, 60, 20).contains("재인증 필요"));
    }
}

