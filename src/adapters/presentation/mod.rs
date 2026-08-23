pub(crate) mod history;
pub(crate) mod model;
pub(crate) mod plain;
pub(crate) mod tui;
pub(crate) mod web;

use crate::application::AgentResult;
use crate::domain::usage::{OriginKind, Severity, UsageSnapshot};

/// 색 사용 여부. `--no-color`, `NO_COLOR`, 비-TTY 를 모두 고려한다.
pub(crate) fn use_color(flag_no_color: bool, is_tty: bool) -> bool {
    if flag_no_color || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    is_tty
}

pub(crate) fn ansi_for(level: Severity) -> &'static str {
    match level {
        Severity::Normal => "\x1b[38;5;147m",   // 연보라
        Severity::Warning => "\x1b[38;5;179m",  // 호박색
        Severity::Critical => "\x1b[38;5;203m", // 적색
    }
}

pub(crate) const DIM: &str = "\x1b[38;5;239m";
pub(crate) const MUTED: &str = "\x1b[38;5;245m";
pub(crate) const BOLD: &str = "\x1b[1m";
pub(crate) const RESET: &str = "\x1b[0m";

/// 여러 에이전트를 담은 `--json` 출력.
/// 에이전트가 하나면 기존 형태를 그대로 유지해 스크립트가 깨지지 않게 한다.
pub(crate) fn to_json_panes(panes: &[AgentResult], timezone: &str) -> anyhow::Result<String> {
    if let [pane] = panes {
        return match &pane.result {
            Ok(snap) => to_json(snap, timezone),
            Err(e) => Err(anyhow::anyhow!("{e}")),
        };
    }
    let mut out = serde_json::Map::new();
    for pane in panes {
        let value = match &pane.result {
            Ok(snap) => serde_json::from_str(&to_json(snap, timezone)?)?,
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        };
        out.insert(pane.agent.name.to_string(), value);
    }
    Ok(serde_json::to_string_pretty(&out)?)
}

fn pct(fill: f64) -> f64 {
    (fill * 1000.0).round() / 10.0
}

fn level_name(level: Severity) -> &'static str {
    match level {
        Severity::Normal => "normal",
        Severity::Warning => "warning",
        Severity::Critical => "critical",
    }
}

fn origin_name(origin: OriginKind) -> &'static str {
    match origin {
        OriginKind::Cache => "cache",
        OriginKind::Live => "live",
    }
}

/// 단일 agent의 `--json` 출력.
fn to_json(snapshot: &UsageSnapshot, timezone: &str) -> anyhow::Result<String> {
    #[derive(serde::Serialize)]
    struct Row<'a> {
        title: &'a str,
        percent: f64,
        label: &'a str,
        level: &'static str,
        emphasized: bool,
        footnote: Option<&'a str>,
        /// 창의 시간이 얼마나 흘렀는지 (0~100). 창 길이를 모르면 없다.
        window_elapsed_percent: Option<f64>,
        /// `1 hour 12 minutes left`
        time_left: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        quota: Option<Quota<'a>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        quota_summary: Option<&'a str>,
    }
    #[derive(serde::Serialize)]
    struct Quota<'a> {
        used: f64,
        limit: f64,
        remaining: f64,
        unit: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        overage_enabled: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        safe_daily_budget: Option<f64>,
    }
    #[derive(serde::Serialize)]
    struct Out<'a> {
        /// 값을 언제 어디서 가져왔는지 — 신선도 판단에 쓴다
        captured_at: String,
        source: &'static str,
        stale: bool,
        limits: Vec<Row<'a>>,
    }
    let meters = model::project(snapshot, timezone, chrono::Local::now());
    let rows: Vec<Row> = meters
        .iter()
        .map(|m| Row {
            title: &m.title,
            percent: pct(m.usage.fill_clamped()),
            label: &m.usage.label,
            level: level_name(m.usage.level),
            emphasized: m.emphasized,
            footnote: m.footnote.as_deref(),
            window_elapsed_percent: m.time.as_ref().map(|t| pct(t.fill_clamped())),
            time_left: m.time.as_ref().map(|t| t.label.as_str()),
            quota: m.quota.as_ref().map(|quota| Quota {
                used: quota.used,
                limit: quota.limit,
                remaining: quota.remaining(),
                unit: &quota.unit,
                overage_enabled: quota.overage_enabled,
                safe_daily_budget: m.safe_daily_budget,
            }),
            quota_summary: m.quota_summary.as_deref(),
        })
        .collect();
    let out = Out {
        captured_at: snapshot.origin.at.to_rfc3339(),
        source: origin_name(snapshot.origin.kind),
        stale: snapshot.origin.refresh_failed,
        limits: rows,
    };
    Ok(serde_json::to_string_pretty(&out)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::usage::{Severity, UsageLimit, UsageSnapshot};

    #[test]
    fn json_keeps_the_public_level_and_source_words() {
        let json = to_json(
            &UsageSnapshot::live(
                vec![UsageLimit::new(
                    "weekly:all",
                    None,
                    75.0,
                    Some(Severity::Warning),
                    false,
                    Some(chrono::TimeDelta::days(7)),
                    None,
                )],
                chrono::Local::now(),
            ),
            "Asia/Seoul",
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["source"], "live");
        assert_eq!(value["limits"][0]["level"], "warning");
    }

    #[test]
    fn json_exposes_raw_quota_when_the_provider_has_it() {
        use crate::domain::usage::UsageQuota;

        let limit = UsageLimit::new(
            "monthly:credits",
            Some("KIRO POWER".into()),
            2.5,
            None,
            true,
            Some(chrono::TimeDelta::days(31)),
            None,
        )
        .with_quota(UsageQuota::new(250.0, 10_000.0, "credits"));
        let value: serde_json::Value = serde_json::from_str(
            &to_json(
                &UsageSnapshot::live(vec![limit], chrono::Local::now()),
                "Asia/Seoul",
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(value["limits"][0]["quota"]["remaining"], 9750.0);
        assert_eq!(value["limits"][0]["quota"]["unit"], "credits");
    }
}
