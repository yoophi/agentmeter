//! Watch 상태와 시간축 의미를 웹 대시보드용 JSON contract로 투영한다.

use std::collections::BTreeMap;

use chrono::{DateTime, Local, Timelike};
use serde::Serialize;

use crate::application::{UsageSample, WatchState};
use crate::domain::usage::{OriginKind, Severity, UsageWindow};

use super::model;

pub(crate) const INDEX: &str = include_str!("web.html");

#[derive(Debug, Serialize)]
pub(crate) struct Dashboard {
    pub timezone: String,
    pub generated_at: String,
    pub next_refresh_at: Option<String>,
    pub refreshing: bool,
    pub panes: Vec<Pane>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Pane {
    pub name: &'static str,
    pub display: &'static str,
    pub origin: Option<String>,
    pub source: Option<&'static str>,
    pub error: Option<String>,
    pub meters: Vec<Meter>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Meter {
    pub title: String,
    pub delta: Option<String>,
    pub used_percent: f64,
    pub label: String,
    pub level: &'static str,
    pub emphasized: bool,
    pub reset: Option<String>,
    pub window: Option<Window>,
    pub chart: Chart,
    pub quota_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<Quota>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Quota {
    pub used: f64,
    pub limit: f64,
    pub remaining: f64,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overage_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_daily_budget: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Window {
    pub started_at: i64,
    pub resets_at: i64,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct Chart {
    pub area_path: String,
    pub line_path: String,
    pub markers: Vec<Marker>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Marker {
    pub x: f64,
    pub kind: &'static str,
}

pub(crate) fn project(
    state: &WatchState,
    timezone: &str,
    now: DateTime<Local>,
    next_refresh_at: Option<DateTime<Local>>,
    refreshing: bool,
) -> Dashboard {
    let panes = state
        .panes()
        .iter()
        .map(|pane| {
            let meters = pane
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    model::project(snapshot, timezone, now)
                        .into_iter()
                        .map(|meter| {
                            let samples = pane.samples(&meter.id, meter.window);
                            Meter {
                                title: meter.title,
                                delta: super::history::delta(samples),
                                used_percent: meter.usage.fill_clamped() * 100.0,
                                label: meter.usage.label,
                                level: level_name(meter.usage.level),
                                emphasized: meter.emphasized,
                                reset: meter.footnote,
                                window: meter.window.map(project_window),
                                chart: meter
                                    .window
                                    .map(|window| project_chart(samples, window))
                                    .unwrap_or_default(),
                                quota_summary: meter.quota_summary,
                                quota: meter.quota.map(|quota| Quota {
                                    used: quota.used,
                                    limit: quota.limit,
                                    remaining: quota.remaining(),
                                    unit: quota.unit,
                                    overage_enabled: quota.overage_enabled,
                                    safe_daily_budget: meter.safe_daily_budget,
                                }),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            Pane {
                name: pane.agent.name,
                display: pane.agent.display,
                origin: pane
                    .snapshot
                    .as_ref()
                    .map(|snapshot| model::origin_text(snapshot.origin, now)),
                source: pane
                    .snapshot
                    .as_ref()
                    .map(|snapshot| match snapshot.origin.kind {
                        OriginKind::Cache => "cache",
                        OriginKind::Live => "live",
                    }),
                error: pane.error.clone(),
                meters,
            }
        })
        .collect();

    Dashboard {
        timezone: timezone.to_string(),
        generated_at: now.to_rfc3339(),
        next_refresh_at: next_refresh_at.map(|at| at.to_rfc3339()),
        refreshing,
        panes,
    }
}

fn project_window(window: UsageWindow) -> Window {
    Window {
        started_at: window.started_at().timestamp(),
        resets_at: window.resets_at.timestamp(),
    }
}

fn project_chart(samples: &[UsageSample], window: UsageWindow) -> Chart {
    let started_at = window.started_at().timestamp();
    let resets_at = window.resets_at.timestamp();
    let span = resets_at - started_at;
    if span <= 0 {
        return Chart::default();
    }

    let mut boundaries = BTreeMap::new();
    for at in hour_boundaries(window) {
        boundaries.insert(at, "hour");
    }
    for at in midnight_boundaries(window) {
        boundaries.insert(at, "midnight");
    }
    let markers = boundaries
        .into_iter()
        .map(|(at, kind)| Marker {
            x: normalized_x(at, started_at, span),
            kind,
        })
        .collect();

    let inside: Vec<UsageSample> = samples
        .iter()
        .copied()
        .filter(|sample| {
            let at = sample.minute * 60;
            at >= started_at && at < resets_at
        })
        .collect();

    // 기록이 끊긴 구간을 이어 그리면 없는 측정을 있는 것처럼 보이게 하므로,
    // 구간마다 subpath를 따로 만들어 공백은 비워 둔다.
    let mut lines = Vec::new();
    let mut areas = Vec::new();
    for run in super::history::segments(&inside) {
        let points: Vec<(f64, f64)> = run
            .iter()
            .map(|sample| {
                (
                    normalized_x(sample.minute * 60, started_at, span),
                    47.0 - sample.percent.clamp(0.0, 100.0) * 0.43,
                )
            })
            .collect();
        let [(x, y)] = points[..] else {
            if points.is_empty() {
                continue;
            }
            let line = points
                .iter()
                .enumerate()
                .map(|(index, (x, y))| {
                    format!("{}{x:.1} {y:.1}", if index == 0 { "M" } else { "L" })
                })
                .collect::<Vec<_>>()
                .join(" ");
            let first_x = points[0].0;
            let last_x = points[points.len() - 1].0;
            areas.push(format!(
                "M{first_x:.1} 48 L{} L{last_x:.1} 48 Z",
                line.strip_prefix('M').unwrap_or(&line)
            ));
            lines.push(line);
            continue;
        };
        // 공백 뒤 첫 표본처럼 구간에 점이 하나뿐이면 면은 폭이 0이라 보이지 않는다.
        // 길이 0인 subpath를 두면 `stroke-linecap: round`가 점으로 그려 준다.
        lines.push(format!("M{x:.1} {y:.1} L{x:.1} {y:.1}"));
    }
    if lines.is_empty() {
        return Chart {
            markers,
            ..Chart::default()
        };
    }

    Chart {
        area_path: areas.join(" "),
        line_path: lines.join(" "),
        markers,
    }
}

fn normalized_x(at: i64, started_at: i64, span: i64) -> f64 {
    ((at - started_at) as f64 / span as f64).clamp(0.0, 1.0) * 1000.0
}

fn midnight_boundaries(window: UsageWindow) -> Vec<i64> {
    let mut midnights = Vec::new();
    let mut date = window.started_at().date_naive().succ_opt();
    while let Some(current) = date {
        let Some(naive) = current.and_hms_opt(0, 0, 0) else {
            break;
        };
        let Some(midnight) = naive.and_local_timezone(Local).earliest() else {
            date = current.succ_opt();
            continue;
        };
        if midnight >= window.resets_at {
            break;
        }
        midnights.push(midnight.timestamp());
        date = current.succ_opt();
    }
    midnights
}

fn hour_boundaries(window: UsageWindow) -> Vec<i64> {
    if window.duration > chrono::TimeDelta::hours(6) {
        return Vec::new();
    }
    let start = window.started_at();
    let Some(mut hour) = start
        .with_minute(0)
        .and_then(|at| at.with_second(0))
        .and_then(|at| at.with_nanosecond(0))
        .map(|at| at + chrono::TimeDelta::hours(1))
    else {
        return Vec::new();
    };
    let mut boundaries = Vec::new();
    while hour < window.resets_at {
        boundaries.push(hour.timestamp());
        hour += chrono::TimeDelta::hours(1);
    }
    boundaries
}

fn level_name(level: Severity) -> &'static str {
    match level {
        Severity::Normal => "normal",
        Severity::Warning => "warning",
        Severity::Critical => "critical",
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;
    use crate::application::{AgentInfo, AgentResult, WatchState};
    use crate::domain::usage::{UsageLimit, UsageSnapshot};

    fn agent(name: &'static str) -> AgentInfo {
        AgentInfo {
            name,
            display: if name == "claude" {
                "Claude Code"
            } else {
                "Codex"
            },
        }
    }

    fn snapshot(percent: f64, at: DateTime<Local>, reset: DateTime<Local>) -> UsageSnapshot {
        UsageSnapshot::live(
            vec![UsageLimit::new(
                "weekly:all",
                None,
                percent,
                None,
                false,
                Some(TimeDelta::days(7)),
                Some(reset),
            )],
            at,
        )
    }

    #[test]
    fn projects_one_or_two_panes_from_watch_state() {
        let now = Local::now();
        let reset = now + TimeDelta::days(2);
        let agents = vec![agent("claude"), agent("codex")];
        let mut state = WatchState::new(agents.clone());
        state.apply(
            agents
                .iter()
                .map(|agent| AgentResult {
                    agent: *agent,
                    result: Ok(snapshot(42.0, now, reset)),
                })
                .collect(),
        );
        let dashboard = project(&state, "Asia/Seoul", now, None, false);
        assert_eq!(dashboard.panes.len(), 2);
        assert_eq!(dashboard.panes[0].meters[0].used_percent, 42.0);
        assert_eq!(dashboard.panes[1].display, "Codex");

        let single = WatchState::new(vec![agent("claude")]);
        assert_eq!(
            project(&single, "Asia/Seoul", now, None, false).panes.len(),
            1
        );
    }

    #[test]
    fn history_is_projected_to_svg_paths_in_rust() {
        let now = Local::now();
        let reset = now + TimeDelta::days(2);
        let info = agent("claude");
        let mut state = WatchState::new(vec![info]);
        for (percent, at) in [(20.0, now - TimeDelta::minutes(1)), (30.0, now)] {
            state.apply(vec![AgentResult {
                agent: info,
                result: Ok(snapshot(percent, at, reset)),
            }]);
        }
        let dashboard = project(&state, "Asia/Seoul", now, None, false);
        let meter = &dashboard.panes[0].meters[0];
        assert!(meter.chart.line_path.starts_with('M'));
        assert!(meter.chart.area_path.ends_with('Z'));
        assert_eq!(meter.delta.as_deref(), Some("+10%p"));
    }

    #[test]
    fn a_recording_gap_leaves_the_area_chart_empty_there() {
        use chrono::TimeZone;

        let window = UsageWindow {
            resets_at: Local
                .with_ymd_and_hms(2026, 8, 20, 12, 0, 0)
                .single()
                .unwrap(),
            duration: TimeDelta::hours(5),
        };
        let start = window.started_at().timestamp() / 60;
        // 창 앞쪽에 5분, 3시간을 쉬고 다시 5분 기록한다.
        let mut samples: Vec<UsageSample> = (0..5)
            .map(|minute| UsageSample {
                minute: start + minute,
                percent: 10.0,
            })
            .collect();
        samples.extend((0..5).map(|minute| UsageSample {
            minute: start + 180 + minute,
            percent: 40.0,
        }));

        let projected = project_chart(&samples, window);
        assert_eq!(
            projected.line_path.matches('M').count(),
            2,
            "기록이 끊긴 자리에서 선도 끊겨야 함: {}",
            projected.line_path
        );
        assert_eq!(
            projected.area_path.matches('Z').count(),
            2,
            "면도 구간마다 닫혀야 함: {}",
            projected.area_path
        );
    }

    #[test]
    fn a_lone_sample_after_a_gap_is_still_drawn() {
        use chrono::TimeZone;

        let window = UsageWindow {
            resets_at: Local
                .with_ymd_and_hms(2026, 8, 20, 12, 0, 0)
                .single()
                .unwrap(),
            duration: TimeDelta::hours(5),
        };
        let start = window.started_at().timestamp() / 60;
        let mut samples: Vec<UsageSample> = (0..5)
            .map(|minute| UsageSample {
                minute: start + minute,
                percent: 10.0,
            })
            .collect();
        // 오래 껐다가 방금 다시 켜서 표본이 하나뿐인 구간.
        samples.push(UsageSample {
            minute: start + 200,
            percent: 40.0,
        });

        let projected = project_chart(&samples, window);
        assert_eq!(
            projected.line_path.matches('M').count(),
            2,
            "표본이 하나인 구간도 남아야 함: {}",
            projected.line_path
        );
        assert_eq!(
            projected.area_path.matches('Z').count(),
            1,
            "점 하나로는 면을 만들 수 없다: {}",
            projected.area_path
        );
    }

    #[test]
    fn continuous_recording_stays_one_path() {
        use chrono::TimeZone;

        let window = UsageWindow {
            resets_at: Local
                .with_ymd_and_hms(2026, 8, 20, 12, 0, 0)
                .single()
                .unwrap(),
            duration: TimeDelta::hours(5),
        };
        let start = window.started_at().timestamp() / 60;
        let samples: Vec<UsageSample> = (0..30)
            .map(|minute| UsageSample {
                minute: start + minute,
                percent: 10.0 + minute as f64,
            })
            .collect();

        let projected = project_chart(&samples, window);
        assert_eq!(projected.line_path.matches('M').count(), 1);
        assert_eq!(projected.area_path.matches('Z').count(), 1);
    }

    #[test]
    fn seven_day_chart_marks_each_local_midnight() {
        use chrono::TimeZone;

        let reset = Local
            .with_ymd_and_hms(2026, 8, 26, 1, 0, 0)
            .single()
            .unwrap();
        let projected = project_chart(
            &[],
            UsageWindow {
                resets_at: reset,
                duration: TimeDelta::days(7),
            },
        );
        assert_eq!(
            projected
                .markers
                .iter()
                .filter(|marker| marker.kind == "midnight")
                .count(),
            7
        );
        assert!(projected.markers.iter().all(|marker| marker.kind != "hour"));
    }

    #[test]
    fn five_hour_chart_marks_full_hours_with_midnight_precedence() {
        use chrono::TimeZone;

        let reset = Local
            .with_ymd_and_hms(2026, 8, 20, 1, 50, 0)
            .single()
            .unwrap();
        let projected = project_chart(
            &[],
            UsageWindow {
                resets_at: reset,
                duration: TimeDelta::hours(5),
            },
        );
        assert_eq!(projected.markers.len(), 5);
        assert_eq!(
            projected
                .markers
                .iter()
                .filter(|marker| marker.kind == "midnight")
                .count(),
            1
        );
    }
}
