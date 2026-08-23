//! 외부 공급자와 화면 표현에 독립적인 사용량 도메인.

use chrono::{DateTime, Local, TimeDelta};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LimitId(String);

impl LimitId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

impl Severity {
    pub fn from_used_percent(percent: f64) -> Self {
        if percent >= 90.0 {
            Self::Critical
        } else if percent >= 75.0 {
            Self::Warning
        } else {
            Self::Normal
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageWindow {
    pub resets_at: DateTime<Local>,
    pub duration: TimeDelta,
}

impl UsageWindow {
    pub fn started_at(&self) -> DateTime<Local> {
        self.resets_at - self.duration
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageLimit {
    pub id: LimitId,
    pub scope: Option<String>,
    pub used_percent: f64,
    pub severity: Severity,
    pub active: bool,
    pub window_duration: Option<TimeDelta>,
    pub resets_at: Option<DateTime<Local>>,
    pub quota: Option<UsageQuota>,
}

/// 공급자가 숫자로 공개하는 사용량과 총 한도.
///
/// 퍼센트만 제공하는 공급자도 있으므로 `UsageLimit`에서는 선택값이다.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageQuota {
    pub used: f64,
    pub limit: f64,
    pub unit: String,
    pub overage_enabled: Option<bool>,
}

impl UsageQuota {
    pub fn new(used: f64, limit: f64, unit: impl Into<String>) -> Self {
        Self {
            used: used.max(0.0),
            limit: limit.max(0.0),
            unit: unit.into(),
            overage_enabled: None,
        }
    }

    pub fn with_overage(mut self, enabled: Option<bool>) -> Self {
        self.overage_enabled = enabled;
        self
    }

    pub fn remaining(&self) -> f64 {
        (self.limit - self.used).max(0.0)
    }
}

impl UsageLimit {
    pub fn new(
        id: impl Into<String>,
        scope: Option<String>,
        used_percent: f64,
        severity: Option<Severity>,
        active: bool,
        window_duration: Option<TimeDelta>,
        resets_at: Option<DateTime<Local>>,
    ) -> Self {
        let used_percent = if used_percent.is_nan() {
            0.0
        } else {
            used_percent.clamp(0.0, 100.0)
        };
        Self {
            id: LimitId::new(id),
            scope,
            used_percent,
            severity: severity.unwrap_or_else(|| Severity::from_used_percent(used_percent)),
            active,
            window_duration,
            resets_at,
            quota: None,
        }
    }

    pub fn with_quota(mut self, quota: UsageQuota) -> Self {
        self.quota = Some(quota);
        self
    }

    pub fn used_fraction(&self) -> f64 {
        self.used_percent / 100.0
    }

    pub fn window(&self) -> Option<UsageWindow> {
        Some(UsageWindow {
            resets_at: self.resets_at?,
            duration: self.window_duration?,
        })
        .filter(|window| window.duration > TimeDelta::zero())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginKind {
    Cache,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin {
    pub at: DateTime<Local>,
    pub kind: OriginKind,
    pub refresh_failed: bool,
}

impl Origin {
    pub fn live(at: DateTime<Local>) -> Self {
        Self {
            at,
            kind: OriginKind::Live,
            refresh_failed: false,
        }
    }

    pub fn cache(at: DateTime<Local>, refresh_failed: bool) -> Self {
        Self {
            at,
            kind: OriginKind::Cache,
            refresh_failed,
        }
    }

    pub fn age_seconds(&self, now: DateTime<Local>) -> i64 {
        (now - self.at).num_seconds().max(0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageSnapshot {
    pub limits: Vec<UsageLimit>,
    pub origin: Origin,
}

impl UsageSnapshot {
    pub fn live(limits: Vec<UsageLimit>, captured_at: DateTime<Local>) -> Self {
        Self {
            limits,
            origin: Origin::live(captured_at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 8, 19, hour, 0, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn usage_is_clamped_and_severity_is_derived() {
        let limit = UsageLimit::new("weekly", None, 175.0, None, false, None, None);
        assert_eq!(limit.used_percent, 100.0);
        assert_eq!(limit.used_fraction(), 1.0);
        assert_eq!(limit.severity, Severity::Critical);
    }

    #[test]
    fn window_requires_duration_and_reset() {
        let limit = UsageLimit::new(
            "session",
            None,
            50.0,
            None,
            false,
            Some(TimeDelta::hours(5)),
            Some(at(12)),
        );
        assert_eq!(limit.window().unwrap().started_at(), at(7));
    }

    #[test]
    fn origin_age_uses_the_supplied_clock() {
        let origin = Origin::cache(at(7), false);
        assert_eq!(origin.age_seconds(at(9)), 7200);
        assert_eq!(origin.age_seconds(at(6)), 0);
    }

    #[test]
    fn quota_never_reports_a_negative_remaining_amount() {
        let quota = UsageQuota::new(12.5, 10.0, "credits");
        assert_eq!(quota.remaining(), 0.0);
    }
}
