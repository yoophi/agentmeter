//! `watch`, 파이프, 일회성 실행용 stdout 렌더러.
//!
//! ratatui 는 alternate screen + raw mode 를 쓰기 때문에 `watch` 아래에서는
//! 동작하지 않는다. stdout 이 TTY 가 아니면 항상 이쪽으로 온다.

use crate::meter::{Bar, Level, Meter, Snapshot};
use crate::render::{BOLD, DIM, MUTED, RESET, ansi_for};

const GAUGE_MIN: usize = 20;
const GAUGE_MAX: usize = 48;
/// 게이지 우측 라벨(`100% used`)이 들어갈 여백
const SUFFIX_ROOM: usize = 14;

pub fn render(snap: &Snapshot, color: bool, width: usize) -> String {
    let gauge_width = width
        .saturating_sub(SUFFIX_ROOM)
        .clamp(GAUGE_MIN, GAUGE_MAX);

    let mut out = String::new();
    for (i, m) in snap.meters.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&render_one(m, color, gauge_width));
    }
    // 값이 언제 기준인지 항상 밝힌다 — 캐시를 읽었을 수도 있기 때문이다
    let (mu, r) = if color { (MUTED, RESET) } else { ("", "") };
    out.push_str(&format!("\n  {mu}{}{r}\n", snap.origin.text()));
    out
}

fn render_one(m: &Meter, color: bool, gauge_width: usize) -> String {
    let (b, mu, r) = if color { (BOLD, MUTED, RESET) } else { ("", "", "") };
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

    let (d, mu, r) = if color { (DIM, MUTED, RESET) } else { ("", "", "") };
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

/// 오류도 같은 형식으로 보여준다 — `watch` 화면이 빈 채로 남지 않게.
pub fn render_error(prog: &str, msg: &str, color: bool) -> String {
    let (a, r) = if color {
        (ansi_for(Level::Critical), RESET)
    } else {
        ("", "")
    };
    format!("{a}{prog}: {msg}{r}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(fill: f64, emphasized: bool) -> Snapshot {
        Snapshot::live(vec![meter(fill, emphasized)])
    }

    fn meter(fill: f64, emphasized: bool) -> Meter {
        Meter {
            title: "Current session".into(),
            usage: Bar {
                fill,
                label: format!("{:.0}% used", fill * 100.0),
                level: Level::Normal,
            },
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
            level: Level::Normal,
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
        let out = render(&snap(0.5, false), false, 200);
        let (filled, empty) = gauge_counts(&out);
        assert_eq!(filled + empty, GAUGE_MAX, "게이지 전체 폭");
        assert_eq!(filled, GAUGE_MAX / 2, "0.5 는 절반만 채워져야 함");
    }

    #[test]
    fn gauge_is_not_always_full() {
        for (fill, want) in [(0.0, 0), (0.25, 12), (1.0, 48)] {
            let out = render(&snap(fill, false), false, 200);
            assert_eq!(gauge_counts(&out).0, want, "fill={fill}");
        }
    }

    #[test]
    fn no_color_emits_no_escape_codes() {
        let out = render(&snap(0.5, false), false, 80);
        assert!(!out.contains('\x1b'), "색 비활성화 시 escape 코드가 없어야 함");
    }

    #[test]
    fn out_of_range_fill_does_not_panic() {
        for fill in [-0.5, 1.5, f64::NAN] {
            let _ = render(&snap(fill, false), true, 80);
        }
    }

    /// 시간 게이지가 있으면 사용량 바로 아래에 한 줄 더 그린다.
    #[test]
    fn time_bar_is_drawn_under_usage() {
        let out = render(&Snapshot::live(vec![meter_with_time(0.7, 0.4)]), false, 200);
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
        let out = render(&snap(0.5, false), false, 200);
        let bars = out
            .lines()
            .filter(|l| l.contains('█') || l.contains('░'))
            .count();
        assert_eq!(bars, 1);
    }

    #[test]
    fn narrow_terminal_still_renders() {
        let out = render(&snap(0.5, false), false, 10);
        assert!(out.contains("50% used"));
    }

    /// 제목·게이지·각주가 같은 열에서 시작해야 한다.
    /// 강조 마커(`›`)가 붙어도 어긋나면 안 된다.
    #[test]
    fn title_aligns_with_gauge() {
        for emphasized in [true, false] {
            let out = render(&snap(0.5, emphasized), false, 200);
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
