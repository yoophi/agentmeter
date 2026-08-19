//! `watch`, 파이프, 일회성 실행용 stdout 렌더러.
//!
//! ratatui 는 alternate screen + raw mode 를 쓰기 때문에 `watch` 아래에서는
//! 동작하지 않는다. stdout 이 TTY 가 아니면 항상 이쪽으로 온다.

use super::model::{self, Bar, Meter};
use super::{BOLD, DIM, MUTED, RESET, ansi_for};
use crate::application::AgentResult;
use crate::domain::usage::{Severity, UsageSnapshot};

const GAUGE_MIN: usize = 20;
const GAUGE_MAX: usize = 48;
/// 게이지 우측 라벨(`100% used`)이 들어갈 자리 + 오른쪽 여백.
/// 게이지가 터미널 경계에 붙으면 답답하다.
const SUFFIX_ROOM: usize = 14 + RIGHT_MARGIN;

/// 화면 오른쪽 여백.
const RIGHT_MARGIN: usize = 2;

pub(crate) fn render(
    snapshot: &UsageSnapshot,
    timezone: &str,
    color: bool,
    width: usize,
) -> String {
    render_at(snapshot, timezone, color, width, chrono::Local::now())
}

fn render_at(
    snapshot: &UsageSnapshot,
    timezone: &str,
    color: bool,
    width: usize,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let meters = model::project(snapshot, timezone, now);
    render_projected(
        &meters,
        &model::origin_text(snapshot.origin, now),
        color,
        width,
    )
}

fn render_projected(meters: &[Meter], origin: &str, color: bool, width: usize) -> String {
    let gauge_width = width
        .saturating_sub(SUFFIX_ROOM)
        .clamp(GAUGE_MIN, GAUGE_MAX);

    let mut out = String::new();
    for (i, m) in meters.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&render_one(m, color, gauge_width));
    }
    // 값이 언제 기준인지 항상 밝힌다 — 캐시를 읽었을 수도 있기 때문이다
    let (mu, r) = if color { (MUTED, RESET) } else { ("", "") };
    out.push_str(&format!("\n  {mu}{origin}{r}\n"));
    out
}

fn render_one(m: &Meter, color: bool, gauge_width: usize) -> String {
    let (b, mu, r) = if color {
        (BOLD, MUTED, RESET)
    } else {
        ("", "", "")
    };
    let marker = if m.emphasized { "› " } else { "  " };

    let mut s = String::new();
    // 제목 — 게이지·각주 줄과 같은 열에 맞춘다
    s.push_str(&format!("{b}{marker}{}{r}\n", m.title));

    s.push_str(&render_bar(&m.usage, color, gauge_width));
    // 시간 게이지는 사용량 바로 아래에 둔다 — 둘을 견줘야 페이스가 읽힌다
    if let Some(time) = &m.time {
        s.push_str(&render_bar(time, color, gauge_width));
    }
    if let Some(note) = &m.footnote {
        s.push_str(&format!("  {mu}{note}{r}\n"));
    }
    s
}

fn render_bar(bar: &Bar, color: bool, gauge_width: usize) -> String {
    let filled = (bar.fill_clamped() * gauge_width as f64).round() as usize;

    let (d, mu, r) = if color {
        (DIM, MUTED, RESET)
    } else {
        ("", "", "")
    };
    let a = if color { ansi_for(bar.level) } else { "" };
    // 색이 꺼진 환경에서는 색만으로 채움/빈칸을 구분할 수 없으므로 글자를 바꾼다
    let empty_ch = if color { "█" } else { "░" };

    let mut s = String::from("  ");
    s.push_str(a);
    s.push_str(&"█".repeat(filled));
    s.push_str(r);
    s.push_str(d);
    s.push_str(&empty_ch.repeat(gauge_width - filled));
    s.push_str(r);
    s.push_str(&format!("  {mu}{}{r}\n", bar.label));
    s
}

/// 구획을 좌우로 나란히 붙이는 데 필요한 최소 폭.
/// 이보다 좁으면 게이지가 쓸모없이 짧아지므로 세로로 쌓는다.
const SIDE_BY_SIDE_MIN: usize = 100;

/// 구획 사이 여백.
const PANE_GAP: usize = 2;

/// 여러 에이전트를 나란히 붙인다.
///
/// 터미널이 충분히 넓으면 TUI 와 같이 **좌우로**(`A | B`) 놓고,
/// 좁으면 세로로 쌓는다 — 폭 40 짜리 게이지 두 개보다 폭 80 짜리 하나가 읽기 쉽다.
pub(crate) fn render_panes(
    panes: &[AgentResult],
    timezone: &str,
    color: bool,
    width: usize,
) -> String {
    let blocks: Vec<String> = panes
        .iter()
        .map(|p| {
            let per = if side_by_side(panes.len(), width) {
                column_width(panes.len(), width)
            } else {
                width
            };
            render_pane(p, timezone, color, per)
        })
        .collect();

    if side_by_side(panes.len(), width) {
        join_columns(&blocks, column_width(panes.len(), width))
    } else {
        blocks.join("\n")
    }
}

fn side_by_side(count: usize, width: usize) -> bool {
    count > 1 && width >= SIDE_BY_SIDE_MIN
}

fn column_width(count: usize, width: usize) -> usize {
    let gaps = PANE_GAP * (count - 1);
    width.saturating_sub(gaps) / count
}

fn render_pane(pane: &AgentResult, timezone: &str, color: bool, width: usize) -> String {
    let (b, r) = if color { (BOLD, RESET) } else { ("", "") };
    let mut out = format!("{b}[{}]{r}\n", pane.agent.display);
    match &pane.result {
        Ok(snap) => out.push_str(&render(snap, timezone, color, width)),
        // 하나가 실패해도 나머지는 계속 보여준다
        Err(e) => out.push_str(&render_error(pane.agent.name, &e.to_string(), color)),
    }
    out
}

/// 여러 블록을 좌우로 합친다. 줄 수가 다르면 짧은 쪽을 빈 줄로 채운다.
fn join_columns(blocks: &[String], col: usize) -> String {
    let columns: Vec<Vec<&str>> = blocks.iter().map(|b| b.lines().collect()).collect();
    let height = columns.iter().map(Vec::len).max().unwrap_or(0);

    (0..height)
        .map(|row| {
            let cells: Vec<String> = columns
                .iter()
                .map(|lines| {
                    let line = lines.get(row).copied().unwrap_or("");
                    pad_to(line, col)
                })
                .collect();
            // 마지막 열 뒤의 공백은 지운다 — 줄 끝 공백은 복사할 때 거슬린다
            cells.join(&" ".repeat(PANE_GAP)).trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// 표시 폭을 기준으로 오른쪽을 채운다. ANSI escape 는 폭이 0 이다.
fn pad_to(line: &str, col: usize) -> String {
    let shown = display_width(line);
    if shown >= col {
        return line.to_string();
    }
    format!("{line}{}", " ".repeat(col - shown))
}

fn display_width(line: &str) -> usize {
    let mut width = 0;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // escape 시퀀스는 화면을 차지하지 않으므로 끝까지 건너뛴다
            for e in chars.by_ref() {
                if e.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        width += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
    }
    width
}

/// 오류도 같은 형식으로 보여준다 — `watch` 화면이 빈 채로 남지 않게.
pub(crate) fn render_error(prog: &str, msg: &str, color: bool) -> String {
    let (a, r) = if color {
        (ansi_for(Severity::Critical), RESET)
    } else {
        ("", "")
    };
    format!("{a}{prog}: {msg}{r}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::AgentInfo;
    use crate::domain::usage::{LimitId, Severity, UsageLimit, UsageSnapshot};

    const TZ: &str = "Asia/Seoul";

    fn agent(name: &'static str) -> AgentInfo {
        AgentInfo {
            name,
            display: match name {
                "claude" => "Claude Code",
                "codex" => "Codex",
                _ => name,
            },
        }
    }

    fn snap(fill: f64, emphasized: bool) -> UsageSnapshot {
        UsageSnapshot::live(
            vec![UsageLimit::new(
                "session:all",
                None,
                fill * 100.0,
                Some(Severity::Normal),
                emphasized,
                None,
                None,
            )],
            chrono::Local::now(),
        )
    }

    fn meter(fill: f64, emphasized: bool) -> Meter {
        Meter {
            id: LimitId::new("session:all"),
            title: "Current session".into(),
            usage: Bar {
                fill,
                label: format!("{:.0}% used", fill * 100.0),
                level: Severity::Normal,
            },
            window: None,
            time: None,
            footnote: Some("Resets Aug 18 at 9:29pm (Asia/Seoul)".into()),
            emphasized,
        }
    }

    fn meter_with_time(usage: f64, time: f64) -> Meter {
        let mut m = meter(usage, false);
        m.time = Some(Bar {
            fill: time,
            label: "1 hour 12 minutes left".into(),
            level: Severity::Normal,
        });
        m
    }

    /// 채움 칸만 센다. 이전 버전은 채움+빈칸 합계를 세는 바람에
    /// "게이지가 항상 꽉 차는" 버그를 통과시켰다.
    fn gauge_counts(out: &str) -> (usize, usize) {
        let bar = out.lines().nth(1).unwrap();
        (
            bar.chars().filter(|c| *c == '█').count(),
            bar.chars().filter(|c| *c == '░').count(),
        )
    }

    #[test]
    fn gauge_length_matches_fill() {
        let out = render(&snap(0.5, false), TZ, false, 200);
        let (filled, empty) = gauge_counts(&out);
        assert_eq!(filled + empty, GAUGE_MAX, "게이지 전체 폭");
        assert_eq!(filled, GAUGE_MAX / 2, "0.5 는 절반만 채워져야 함");
    }

    #[test]
    fn gauge_is_not_always_full() {
        for (fill, want) in [(0.0, 0), (0.25, 12), (1.0, 48)] {
            let out = render(&snap(fill, false), TZ, false, 200);
            assert_eq!(gauge_counts(&out).0, want, "fill={fill}");
        }
    }

    #[test]
    fn no_color_emits_no_escape_codes() {
        let out = render(&snap(0.5, false), TZ, false, 80);
        assert!(
            !out.contains('\x1b'),
            "색 비활성화 시 escape 코드가 없어야 함"
        );
    }

    #[test]
    fn out_of_range_fill_does_not_panic() {
        for fill in [-0.5, 1.5, f64::NAN] {
            let _ = render(&snap(fill, false), TZ, true, 80);
        }
    }

    /// 시간 게이지가 있으면 사용량 바로 아래에 한 줄 더 그린다.
    #[test]
    fn time_bar_is_drawn_under_usage() {
        let out = render_projected(
            &[meter_with_time(0.7, 0.4)],
            "기준 12:00 (방금, 직접 조회)",
            false,
            200,
        );
        let bars: Vec<&str> = out
            .lines()
            .filter(|l| l.contains('█') || l.contains('░'))
            .collect();
        assert_eq!(bars.len(), 2, "게이지가 두 줄이어야 함:\n{out}");
        assert!(bars[0].contains("70% used"), "{}", bars[0]);
        assert!(bars[1].contains("1 hour 12 minutes left"), "{}", bars[1]);
    }

    /// 창 길이를 모르면 시간 게이지 없이 사용량만 그린다.
    #[test]
    fn without_time_only_one_bar() {
        let out = render(&snap(0.5, false), TZ, false, 200);
        let bars = out
            .lines()
            .filter(|l| l.contains('█') || l.contains('░'))
            .count();
        assert_eq!(bars, 1);
    }

    /// 넓은 화면에서는 구획을 좌우로 나란히 놓는다.
    #[test]
    fn wide_terminal_puts_panes_side_by_side() {
        let panes = vec![
            AgentResult {
                agent: agent("claude"),
                result: Ok(snap(0.5, false)),
            },
            AgentResult {
                agent: agent("codex"),
                result: Ok(snap(0.25, false)),
            },
        ];
        let out = render_panes(&panes, TZ, false, 160);
        let head = out.lines().next().unwrap();
        assert!(head.contains("[Claude Code]"), "{head}");
        assert!(head.contains("[Codex]"), "같은 줄에 나란히: {head}");
        assert!(
            head.find("[Claude Code]") < head.find("[Codex]"),
            "설정 순서대로 왼쪽부터"
        );
    }

    /// 좁은 화면에서는 세로로 쌓는다 — 게이지가 쓸모없이 짧아지는 것보다 낫다.
    #[test]
    fn narrow_terminal_stacks_panes() {
        let panes = vec![
            AgentResult {
                agent: agent("claude"),
                result: Ok(snap(0.5, false)),
            },
            AgentResult {
                agent: agent("codex"),
                result: Ok(snap(0.25, false)),
            },
        ];
        let out = render_panes(&panes, TZ, false, 60);
        let head = out.lines().next().unwrap();
        assert!(head.contains("[Claude Code]"));
        assert!(
            !head.contains("[Codex]"),
            "좁으면 같은 줄에 두지 않는다: {head}"
        );
        assert!(out.contains("[Codex]"), "아래에 있어야 함");
    }

    /// 줄 끝에 공백을 남기지 않는다.
    #[test]
    fn no_trailing_whitespace_in_columns() {
        let joined = join_columns(&["a\nbb".to_string(), "c".to_string()], 5);
        for line in joined.lines() {
            assert_eq!(line, line.trim_end(), "줄 끝 공백: {line:?}");
        }
    }

    /// 각 구획에 에이전트 이름이 붙고, 하나가 실패해도 나머지는 그려진다.
    #[test]
    fn panes_are_labeled_and_survive_failures() {
        let ok = AgentResult {
            agent: agent("claude"),
            result: Ok(snap(0.5, false)),
        };
        let bad = AgentResult {
            agent: agent("codex"),
            result: Err(crate::application::FetchError::Other(anyhow::anyhow!(
                "조회 실패"
            ))),
        };
        let out = render_panes(&[ok, bad], TZ, false, 80);
        assert!(out.contains("[Claude Code]"), "{out}");
        assert!(out.contains("50% used"), "{out}");
        assert!(out.contains("[Codex]"), "{out}");
        assert!(out.contains("조회 실패"), "실패도 화면에 남아야 함:\n{out}");
    }

    #[test]
    fn narrow_terminal_still_renders() {
        let out = render(&snap(0.5, false), TZ, false, 10);
        assert!(out.contains("50% used"));
    }

    /// 제목·게이지·각주가 같은 열에서 시작해야 한다.
    /// 강조 마커(`›`)가 붙어도 어긋나면 안 된다.
    #[test]
    fn title_aligns_with_gauge() {
        for emphasized in [true, false] {
            let out = render(&snap(0.5, emphasized), TZ, false, 200);
            let mut lines = out.lines();
            let title_line = lines.next().unwrap();
            let gauge_line = lines.next().unwrap();

            // 화면 폭은 문자 단위이므로 바이트가 아닌 char 로 센다
            let col_of = |line: &str, at: usize| line[..at].chars().count();
            let title_col = col_of(title_line, title_line.find("Current").unwrap());
            let gauge_col = col_of(gauge_line, gauge_line.find(['█', '░']).unwrap());

            assert_eq!(title_col, gauge_col, "emphasized={emphasized} 열이 어긋남");
        }
    }
}
