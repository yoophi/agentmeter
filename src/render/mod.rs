pub mod plain;
pub mod tui;

use crate::meter::Level;

/// 색 사용 여부. `--no-color`, `NO_COLOR`, 비-TTY 를 모두 고려한다.
pub fn use_color(flag_no_color: bool, is_tty: bool) -> bool {
    if flag_no_color || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    is_tty
}

pub fn ansi_for(level: Level) -> &'static str {
    match level {
        Level::Normal => "\x1b[38;5;147m",   // 연보라
        Level::Warning => "\x1b[38;5;179m",  // 호박색
        Level::Critical => "\x1b[38;5;203m", // 적색
    }
}

pub const DIM: &str = "\x1b[38;5;239m";
pub const MUTED: &str = "\x1b[38;5;245m";
pub const BOLD: &str = "\x1b[1m";
pub const RESET: &str = "\x1b[0m";

fn pct(fill: f64) -> f64 {
    (fill * 1000.0).round() / 10.0
}

/// `--json` 출력. 두 도구가 같은 형태를 내보낸다.
pub fn to_json(snap: &crate::meter::Snapshot) -> anyhow::Result<String> {
    #[derive(serde::Serialize)]
    struct Row<'a> {
        title: &'a str,
        percent: f64,
        label: &'a str,
        level: crate::meter::Level,
        emphasized: bool,
        footnote: Option<&'a str>,
        /// 창의 시간이 얼마나 흘렀는지 (0~100). 창 길이를 모르면 없다.
        window_elapsed_percent: Option<f64>,
        /// `1 hour 12 minutes left`
        time_left: Option<&'a str>,
    }
    #[derive(serde::Serialize)]
    struct Out<'a> {
        /// 값을 언제 어디서 가져왔는지 — 신선도 판단에 쓴다
        captured_at: String,
        source: crate::meter::OriginKind,
        stale: bool,
        limits: Vec<Row<'a>>,
    }
    let rows: Vec<Row> = snap
        .meters
        .iter()
        .map(|m| Row {
            title: &m.title,
            percent: pct(m.usage.fill_clamped()),
            label: &m.usage.label,
            level: m.usage.level,
            emphasized: m.emphasized,
            footnote: m.footnote.as_deref(),
            window_elapsed_percent: m.time.as_ref().map(|t| pct(t.fill_clamped())),
            time_left: m.time.as_ref().map(|t| t.label.as_str()),
        })
        .collect();
    let out = Out {
        captured_at: snap.origin.at.to_rfc3339(),
        source: snap.origin.kind,
        stale: snap.origin.refresh_failed,
        limits: rows,
    };
    Ok(serde_json::to_string_pretty(&out)?)
}
