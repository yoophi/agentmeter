//! 사용량 도메인을 모든 출력 adapter가 공유하는 화면 모델로 투영한다.

use chrono::{DateTime, Local, TimeDelta};

use crate::domain::usage::{
    LimitId, Origin, OriginKind, Severity, UsageLimit, UsageQuota, UsageSnapshot, UsageWindow,
};

#[derive(Debug, Clone)]
pub(crate) struct Bar {
    pub fill: f64,
    pub label: String,
    pub level: Severity,
}

impl Bar {
    pub fn fill_clamped(&self) -> f64 {
        if self.fill.is_nan() {
            return 0.0;
        }
        self.fill.clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Meter {
    pub id: LimitId,
    pub title: String,
    pub usage: Bar,
    pub window: Option<UsageWindow>,
    pub time: Option<Bar>,
    pub footnote: Option<String>,
    pub emphasized: bool,
    pub quota: Option<UsageQuota>,
    pub safe_daily_budget: Option<f64>,
    pub quota_summary: Option<String>,
}

pub(crate) fn project(
    snapshot: &UsageSnapshot,
    timezone: &str,
    now: DateTime<Local>,
) -> Vec<Meter> {
    snapshot
        .limits
        .iter()
        .map(|limit| project_limit(limit, timezone, now))
        .collect()
}

fn project_limit(limit: &UsageLimit, timezone: &str, now: DateTime<Local>) -> Meter {
    let duration_minutes = limit.window_duration.map(|duration| duration.num_minutes());
    let time = match (limit.resets_at, limit.window_duration) {
        (Some(reset), Some(duration)) => time_bar(reset, duration, now),
        (None, Some(_)) => Some(pending_bar()),
        _ => None,
    };
    let safe_daily_budget = match (&limit.quota, limit.resets_at) {
        (Some(quota), Some(reset)) if reset > now => {
            let days = (reset - now).num_days().max(1) as f64;
            Some(quota.remaining() / days)
        }
        _ => None,
    };
    let quota_summary = limit
        .quota
        .as_ref()
        .map(|quota| quota_summary(quota, safe_daily_budget));
    Meter {
        id: limit.id.clone(),
        title: window_title(duration_minutes, limit.scope.as_deref()),
        usage: Bar {
            fill: limit.used_fraction(),
            label: format!("{:.0}% used", limit.used_percent),
            level: limit.severity,
        },
        window: limit.window(),
        time,
        footnote: limit.resets_at.map(|reset| resets_text(reset, timezone)),
        emphasized: limit.active,
        quota: limit.quota.clone(),
        safe_daily_budget,
        quota_summary,
    }
}

fn quota_summary(quota: &UsageQuota, safe_daily_budget: Option<f64>) -> String {
    let mut parts = vec![format!(
        "{} / {} {} · {} left",
        format_amount(quota.used),
        format_amount(quota.limit),
        quota.unit,
        format_amount(quota.remaining()),
    )];
    if let Some(budget) = safe_daily_budget {
        parts.push(format!("daily budget {}/day", format_amount(budget)));
    }
    if let Some(enabled) = quota.overage_enabled {
        parts.push(format!(
            "Overages {}",
            if enabled { "enabled" } else { "disabled" }
        ));
    }
    parts.join(" · ")
}

fn format_amount(value: f64) -> String {
    let fixed = format!("{value:.2}");
    let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
    let (whole, fraction) = trimmed.split_once('.').unwrap_or((trimmed, ""));
    let mut grouped = String::new();
    for (index, ch) in whole.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let whole: String = grouped.chars().rev().collect();
    if fraction.is_empty() {
        whole
    } else {
        format!("{whole}.{fraction}")
    }
}

pub(crate) fn origin_text(origin: Origin, now: DateTime<Local>) -> String {
    let source = match origin.kind {
        OriginKind::Cache => "로컬 캐시",
        OriginKind::Live => "직접 조회",
    };
    let mut text = format!(
        "기준 {} ({}, {source})",
        origin.at.format("%H:%M"),
        humanize_age(origin.age_seconds(now))
    );
    if origin.refresh_failed {
        text.push_str(" · 갱신 실패");
    }
    text
}

fn humanize_age(seconds: i64) -> String {
    match seconds {
        value if value < 60 => "방금".to_string(),
        value if value < 3600 => format!("{}분 전", value / 60),
        value if value < 86400 => format!("{}시간 전", value / 3600),
        value => format!("{}일 전", value / 86400),
    }
}

fn resets_text(at: DateTime<Local>, timezone: &str) -> String {
    format!("Resets {} ({timezone})", at.format("%b %-d at %-I:%M%P"))
}

fn window_title(duration_minutes: Option<i64>, scope: Option<&str>) -> String {
    let is_session = matches!(duration_minutes, Some(minutes) if minutes <= 60 * 6);
    let base = match duration_minutes {
        _ if is_session => "Current session".to_string(),
        Some(minutes) if minutes <= 60 * 24 => "Current day".to_string(),
        Some(minutes) if minutes <= 60 * 24 * 7 => "Current week".to_string(),
        Some(minutes) if minutes <= 60 * 24 * 31 => "Current month".to_string(),
        Some(minutes) => format!("Current {}h window", minutes / 60),
        None => "Current limit".to_string(),
    };
    match scope {
        None if is_session => base,
        _ => format!("{base} ({})", scope.unwrap_or("all models")),
    }
}

fn time_bar(resets_at: DateTime<Local>, window: TimeDelta, now: DateTime<Local>) -> Option<Bar> {
    if window <= TimeDelta::zero() {
        return None;
    }
    let remaining = (resets_at - now).max(TimeDelta::zero());
    let elapsed = (window - remaining).max(TimeDelta::zero());
    Some(Bar {
        fill: elapsed.num_seconds() as f64 / window.num_seconds() as f64,
        label: time_left_label(remaining, window >= TimeDelta::days(1)),
        level: Severity::Normal,
    })
}

fn pending_bar() -> Bar {
    Bar {
        fill: 0.0,
        label: "not started".to_string(),
        level: Severity::Normal,
    }
}

fn time_left_label(remaining: TimeDelta, with_days: bool) -> String {
    let total = remaining.num_minutes().max(0);
    if with_days {
        let (days, hours, minutes) = (total / 1440, (total % 1440) / 60, total % 60);
        format!("{days} day {hours} hour {minutes} minutes left")
    } else {
        format!("{} hour {} minutes left", total / 60, total % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 8, 18, 21, 0, 0)
            .single()
            .unwrap()
    }

    fn limit(duration: Option<TimeDelta>, reset: Option<DateTime<Local>>) -> UsageLimit {
        UsageLimit::new(
            "weekly:all",
            None,
            75.0,
            Some(Severity::Warning),
            true,
            duration,
            reset,
        )
    }

    #[test]
    fn projection_centralizes_shared_wording() {
        let snapshot = UsageSnapshot::live(
            vec![limit(
                Some(TimeDelta::days(7)),
                Some(now() + TimeDelta::days(2)),
            )],
            now(),
        );
        let meter = &project(&snapshot, "Asia/Seoul", now())[0];
        assert_eq!(meter.title, "Current week (all models)");
        assert_eq!(meter.usage.label, "75% used");
        assert_eq!(
            meter.time.as_ref().unwrap().label,
            "2 day 0 hour 0 minutes left"
        );
        assert_eq!(
            meter.footnote.as_deref(),
            Some("Resets Aug 20 at 9:00pm (Asia/Seoul)")
        );
        assert!(meter.emphasized);
        assert!(meter.quota.is_none());
        assert!(meter.safe_daily_budget.is_none());
        assert!(meter.quota_summary.is_none());
    }

    #[test]
    fn pending_window_keeps_the_second_row() {
        let snapshot = UsageSnapshot::live(vec![limit(Some(TimeDelta::hours(5)), None)], now());
        let meter = &project(&snapshot, "Asia/Seoul", now())[0];
        assert_eq!(meter.title, "Current session");
        assert_eq!(meter.time.as_ref().unwrap().label, "not started");
        assert!(meter.footnote.is_none());
    }

    #[test]
    fn origin_wording_is_presentation_only() {
        let origin = Origin::cache(now() - TimeDelta::minutes(3), true);
        assert_eq!(
            origin_text(origin, now()),
            "기준 20:57 (3분 전, 로컬 캐시) · 갱신 실패"
        );
    }

    #[test]
    fn invalid_usage_and_time_are_clamped() {
        let weird = UsageLimit::new(
            "weird",
            None,
            f64::NAN,
            None,
            false,
            Some(TimeDelta::hours(5)),
            Some(now() + TimeDelta::hours(10)),
        );
        let snapshot = UsageSnapshot::live(vec![weird], now());
        let meter = &project(&snapshot, "Asia/Seoul", now())[0];
        assert_eq!(meter.usage.fill, 0.0);
        assert_eq!(meter.time.as_ref().unwrap().fill, 0.0);
    }

    #[test]
    fn numeric_quota_is_shown_as_credits_and_remaining_balance() {
        let limit = limit(Some(TimeDelta::days(31)), Some(now() + TimeDelta::days(8)))
            .with_quota(UsageQuota::new(271.77, 10_000.0, "credits"));
        let snapshot = UsageSnapshot::live(vec![limit], now());
        let meter = &project(&snapshot, "Asia/Seoul", now())[0];
        assert_eq!(meter.usage.label, "75% used");
        assert!(meter.safe_daily_budget.is_some());
        assert!(
            meter
                .quota_summary
                .as_deref()
                .unwrap()
                .contains("271.77 / 10,000 credits")
        );
        assert!(
            meter
                .quota_summary
                .as_deref()
                .unwrap()
                .contains("daily budget")
        );
    }
}
