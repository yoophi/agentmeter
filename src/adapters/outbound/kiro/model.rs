//! `kiro-cli chat --no-interactive /usage` 텍스트 파싱과 도메인 변환.

use anyhow::{Context, anyhow, bail};
use chrono::{Datelike, Local, NaiveDate, TimeZone};

use crate::domain::usage::{UsageLimit, UsageQuota};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct KiroUsage {
    pub plan: String,
    pub used: f64,
    pub limit: f64,
    pub reset_date: NaiveDate,
    pub overage_enabled: Option<bool>,
}

pub(super) fn parse(raw: &str) -> anyhow::Result<KiroUsage> {
    let text = strip_ansi(raw);
    let header = text
        .lines()
        .find(|line| line.contains("Estimated Usage") && line.contains("resets on"))
        .context("no Estimated Usage header")?;
    let mut reset_date = None;
    let mut plan = None;
    for part in header.split('|').map(str::trim) {
        if let Some(value) = part.strip_prefix("resets on ") {
            reset_date = Some(
                NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
                    .context("the reset date has an unexpected format")?,
            );
        } else if !part.contains("Estimated Usage") && !part.is_empty() {
            plan = Some(part.to_string());
        }
    }

    let credits = text
        .lines()
        .find(|line| line.trim_start().starts_with("Credits"))
        .context("no Credits row")?;
    let inside = credits
        .split_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(inside, _)| inside)
        .context("could not read the Credits usage parentheses")?;
    let (used, limit_and_suffix) = inside
        .split_once(" of ")
        .context("the Credits usage has no `of` separator")?;
    let limit = limit_and_suffix
        .split_whitespace()
        .next()
        .context("no Credit limit")?;
    let used = number(used)?;
    let limit = number(limit)?;
    if limit <= 0.0 {
        bail!("the Credit limit must be greater than 0");
    }

    let overage_enabled = text.lines().find_map(|line| {
        let value = line.trim().strip_prefix("Overages:")?.trim();
        if value.eq_ignore_ascii_case("enabled") {
            Some(true)
        } else if value.eq_ignore_ascii_case("disabled") {
            Some(false)
        } else {
            None
        }
    });

    Ok(KiroUsage {
        plan: plan.context("no Kiro plan name")?,
        used,
        limit,
        reset_date: reset_date.context("no reset date")?,
        overage_enabled,
    })
}

fn number(value: &str) -> anyhow::Result<f64> {
    value
        .trim()
        .replace(',', "")
        .parse()
        .map_err(|_| anyhow!("could not read the number: {value}"))
}

pub(super) fn to_limit(usage: &KiroUsage) -> anyhow::Result<UsageLimit> {
    let reset = local_midnight(usage.reset_date)
        .context("could not convert the reset date to local time")?;
    let previous = previous_month(usage.reset_date)?;
    let started = local_midnight(previous)
        .context("could not convert the previous reset date to local time")?;
    let percent = usage.used / usage.limit * 100.0;
    Ok(UsageLimit::new(
        "monthly:credits",
        Some(usage.plan.clone()),
        percent,
        None,
        true,
        Some(reset - started),
        Some(reset),
    )
    .with_quota(
        UsageQuota::new(usage.used, usage.limit, "credits").with_overage(usage.overage_enabled),
    ))
}

fn local_midnight(date: NaiveDate) -> Option<chrono::DateTime<Local>> {
    let naive = date.and_hms_opt(0, 0, 0)?;
    Local
        .from_local_datetime(&naive)
        .single()
        .or_else(|| Local.from_local_datetime(&naive).earliest())
}

fn previous_month(date: NaiveDate) -> anyhow::Result<NaiveDate> {
    let (year, month) = if date.month() == 1 {
        (date.year() - 1, 12)
    } else {
        (date.year(), date.month() - 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1).context("could not work out the previous billing month")
}

/// 터미널 스타일(CSI/OSC)을 제거해 사람이 보는 출력과 같은 문자열로 만든다.
fn strip_ansi(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            let ch = value[index..].chars().next().expect("valid UTF-8 boundary");
            out.push(ch);
            index += ch.len_utf8();
            continue;
        }
        index += 1;
        if index >= bytes.len() {
            break;
        }
        match bytes[index] {
            b'[' => {
                index += 1;
                while index < bytes.len() {
                    let byte = bytes[index];
                    index += 1;
                    if (0x40..=0x7e).contains(&byte) {
                        break;
                    }
                }
            }
            b']' => {
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == 0x07 {
                        index += 1;
                        break;
                    }
                    if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            _ => index += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colored_power_plan_output() {
        let raw = "\x1b[1mEstimated Usage\x1b[0m | resets on 2026-09-01 | \x1b[mKIRO POWER\x1b[0m\n\
                   \x1b[1mCredits\x1b[0m (271.77 of 10000 covered in plan)\n";
        let usage = parse(raw).unwrap();
        assert_eq!(usage.plan, "KIRO POWER");
        assert_eq!(usage.used, 271.77);
        assert_eq!(usage.limit, 10_000.0);
        assert_eq!(
            usage.reset_date,
            NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
        );
        assert_eq!(usage.overage_enabled, None);
    }

    #[test]
    fn parses_commas_and_overage_state() {
        let usage = parse(
            "Estimated Usage | resets on 2026-10-01 | KIRO PRO+\n\
             Credits (1,234.5 of 2,000 covered in plan)\nOverages: Disabled\n",
        )
        .unwrap();
        assert_eq!(usage.used, 1234.5);
        assert_eq!(usage.limit, 2000.0);
        assert_eq!(usage.overage_enabled, Some(false));
    }

    #[test]
    fn maps_credits_to_a_monthly_domain_limit() {
        let usage = KiroUsage {
            plan: "KIRO POWER".into(),
            used: 250.0,
            limit: 10_000.0,
            reset_date: NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
            overage_enabled: None,
        };
        let limit = to_limit(&usage).unwrap();
        assert_eq!(limit.id.as_str(), "monthly:credits");
        assert_eq!(limit.scope.as_deref(), Some("KIRO POWER"));
        assert_eq!(limit.used_percent, 2.5);
        assert_eq!(limit.quota.as_ref().unwrap().remaining(), 9750.0);
        assert_eq!(limit.window_duration.unwrap().num_days(), 31);
    }
}
