//! 두 도구가 공유하는 화면 표현.
//!
//! ccmeter 는 "얼마나 썼나"(`51% used`)를, codexmeter 는 "얼마나 남았나"
//! (`71% left`)를 보여준다. 그래서 퍼센트 하나로 통일하지 않고,
//! 게이지 채움 비율(`fill`)과 라벨 문자열(`label`)을 따로 둔다.
//! 각 도구가 자기 방식대로 채워 넣고, 렌더러는 그대로 그리기만 한다.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Normal,
    Warning,
    Critical,
}

/// 게이지 한 줄.
#[derive(Debug, Clone)]
pub struct Bar {
    /// 채움 비율 (0.0 ~ 1.0)
    pub fill: f64,
    /// 게이지 안/옆에 붙는 문구 — `51% used`, `1 hour 12 minutes left`
    pub label: String,
    pub level: Level,
}

impl Bar {
    /// 소진율(0~100)로 사용량 게이지를 만든다. 두 도구가 같은 문구·비율을 쓰도록
    /// 여기 모아 둔다. `level` 을 주지 않으면 소진율로 등급을 정한다.
    pub fn used(percent: f64, level: Option<Level>) -> Self {
        let pct = if percent.is_nan() {
            0.0
        } else {
            percent.clamp(0.0, 100.0)
        };
        Bar {
            fill: pct / 100.0,
            label: format!("{pct:.0}% used"),
            level: level.unwrap_or_else(|| level_from_used(pct)),
        }
    }

    pub fn fill_clamped(&self) -> f64 {
        if self.fill.is_nan() {
            return 0.0;
        }
        self.fill.clamp(0.0, 1.0)
    }
}

/// 한도 창(window). 시간 게이지와 시계열 차트의 가로축이 된다.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    pub resets_at: chrono::DateTime<chrono::Local>,
    pub len: chrono::TimeDelta,
}

impl Window {
    pub fn started_at(&self) -> chrono::DateTime<chrono::Local> {
        self.resets_at - self.len
    }
}

#[derive(Debug, Clone)]
pub struct Meter {
    pub title: String,
    /// 한도를 얼마나 썼는지
    pub usage: Bar,
    /// 이 한도가 속한 창. 창 길이를 모르면 없다.
    pub window: Option<Window>,
    /// 창(window)의 시간이 얼마나 흘렀는지. 창 길이를 모르면 없다.
    ///
    /// 사용률과 나란히 두면 페이스를 볼 수 있다 —
    /// 시간은 20% 지났는데 한도를 65% 썼다면 이번 창은 일찍 소진된다.
    pub time: Option<Bar>,
    /// 게이지 아래 한 줄 — `Resets Aug 18 at 9:29pm (Asia/Seoul)`
    pub footnote: Option<String>,
    /// 지금 적용 중인 한도 등, 강조할 항목
    pub emphasized: bool,
}

/// 소진율(0~100)로 등급을 매긴다. 남은 비율을 쓰는 쪽은 `100 - left` 를 넘기면 된다.
pub fn level_from_used(used_percent: f64) -> Level {
    if used_percent >= 90.0 {
        Level::Critical
    } else if used_percent >= 75.0 {
        Level::Warning
    } else {
        Level::Normal
    }
}

/// 리셋 시각 문구. 두 도구가 같은 함수를 써서 표기를 통일한다.
///
/// `Resets Aug 20 at 12:31pm (Asia/Seoul)` — 오늘이든 아니든 날짜를 항상 붙인다.
/// 시각만 있으면 "오늘 9:30pm" 인지 "내일 9:30pm" 인지 화면만 보고는 알 수 없다.
pub fn resets_text(at: chrono::DateTime<chrono::Local>, tz: &str) -> String {
    format!("Resets {} ({tz})", at.format("%b %-d at %-I:%M%P"))
}

/// 창 길이(분)로 제목을 만든다. 두 도구가 같은 문구를 쓰도록 여기 모아 둔다.
///
/// `scope` 는 괄호 안에 들어갈 대상 — 모델명 등. 없으면 `all models`.
pub fn window_title(duration_mins: Option<i64>, scope: Option<&str>) -> String {
    let is_session = matches!(duration_mins, Some(m) if m <= 60 * 6);
    let base = match duration_mins {
        _ if is_session => "Current session".to_string(),
        Some(m) if m <= 60 * 24 => "Current day".to_string(),
        Some(m) if m <= 60 * 24 * 7 => "Current week".to_string(),
        Some(m) if m <= 60 * 24 * 31 => "Current month".to_string(),
        Some(m) => format!("Current {}h window", m / 60),
        None => "Current limit".to_string(),
    };
    // 세션 창은 대상이 하나뿐이라 괄호를 붙이지 않는다
    match scope {
        None if is_session => base,
        _ => format!("{base} ({})", scope.unwrap_or("all models")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_titles_match_ccmeter_wording() {
        assert_eq!(window_title(Some(300), None), "Current session");
        assert_eq!(window_title(Some(10080), None), "Current week (all models)");
        assert_eq!(
            window_title(Some(10080), Some("GPT-5.3-Codex-Spark")),
            "Current week (GPT-5.3-Codex-Spark)"
        );
    }

    #[test]
    fn unknown_window_still_gets_a_title() {
        assert!(window_title(None, None).starts_with("Current"));
        assert!(window_title(Some(60 * 24 * 90), None).starts_with("Current"));
    }

    /// 세션 한도처럼 오늘 안에 리셋되는 경우에도 날짜가 보여야 한다.
    #[test]
    fn resets_text_always_includes_the_date() {
        use chrono::{Local, TimeZone};
        let at = Local.with_ymd_and_hms(2026, 8, 18, 21, 30, 0).single().unwrap();
        assert_eq!(
            resets_text(at, "Asia/Seoul"),
            "Resets Aug 18 at 9:30pm (Asia/Seoul)"
        );
    }

    #[test]
    fn level_thresholds() {
        assert_eq!(level_from_used(0.0), Level::Normal);
        assert_eq!(level_from_used(74.9), Level::Normal);
        assert_eq!(level_from_used(75.0), Level::Warning);
        assert_eq!(level_from_used(90.0), Level::Critical);
    }
}

/// 값을 어디서 언제 가져왔는지. 화면에 함께 표시해 신선도를 알린다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OriginKind {
    /// 에이전트가 로컬에 남긴 캐시를 읽었다 — 네트워크 호출 없음
    Cache,
    /// 직접 조회했다
    Live,
}

#[derive(Debug, Clone, Copy)]
pub struct Origin {
    pub at: chrono::DateTime<chrono::Local>,
    pub kind: OriginKind,
    /// 갱신을 시도했지만 실패해서 낡은 값을 쓰고 있다
    pub refresh_failed: bool,
}

/// 한 번의 조회 결과.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub meters: Vec<Meter>,
    pub origin: Origin,
}

impl Snapshot {
    /// 방금 직접 조회해서 만든 결과.
    pub fn live(meters: Vec<Meter>) -> Self {
        Snapshot {
            meters,
            origin: Origin::live(),
        }
    }
}

impl Origin {
    pub fn live() -> Self {
        Origin {
            at: chrono::Local::now(),
            kind: OriginKind::Live,
            refresh_failed: false,
        }
    }

    /// 캐시에서 읽은 값. `refresh_failed` 는 갱신을 시도했다가 실패했는지.
    pub fn cache(at: chrono::DateTime<chrono::Local>, refresh_failed: bool) -> Self {
        Origin {
            at,
            kind: OriginKind::Cache,
            refresh_failed,
        }
    }
}

impl Origin {
    /// `기준 20:54 (3분 전, 로컬 캐시)`
    pub fn text(&self) -> String {
        let source = match self.kind {
            OriginKind::Cache => "로컬 캐시",
            OriginKind::Live => "직접 조회",
        };
        let mut s = format!(
            "기준 {} ({}, {source})",
            self.at.format("%H:%M"),
            humanize_age(self.age_secs())
        );
        if self.refresh_failed {
            s.push_str(" · 갱신 실패");
        }
        s
    }

    pub fn age_secs(&self) -> i64 {
        (chrono::Local::now() - self.at).num_seconds().max(0)
    }
}

fn humanize_age(secs: i64) -> String {
    match secs {
        s if s < 60 => "방금".to_string(),
        s if s < 3600 => format!("{}분 전", s / 60),
        s if s < 86400 => format!("{}시간 전", s / 3600),
        s => format!("{}일 전", s / 86400),
    }
}

#[cfg(test)]
mod origin_tests {
    use super::*;
    use chrono::{Duration, Local};

    fn origin(minutes: i64, kind: OriginKind, failed: bool) -> Origin {
        Origin {
            at: Local::now() - Duration::minutes(minutes),
            kind,
            refresh_failed: failed,
        }
    }

    #[test]
    fn shows_source_and_age() {
        let t = origin(3, OriginKind::Cache, false).text();
        assert!(t.contains("3분 전"), "{t}");
        assert!(t.contains("로컬 캐시"), "{t}");

        let t = origin(0, OriginKind::Live, false).text();
        assert!(t.contains("방금"), "{t}");
        assert!(t.contains("직접 조회"), "{t}");
    }

    /// 낡은 값을 보여주는 중이면 그 사실이 드러나야 한다.
    #[test]
    fn marks_failed_refresh() {
        let t = origin(40, OriginKind::Cache, true).text();
        assert!(t.contains("갱신 실패"), "{t}");
        assert!(t.contains("40분 전"), "{t}");
    }

    #[test]
    fn humanizes_long_gaps() {
        assert_eq!(humanize_age(30), "방금");
        assert_eq!(humanize_age(59), "방금");
        assert_eq!(humanize_age(60), "1분 전");
        assert_eq!(humanize_age(3600), "1시간 전");
        assert_eq!(humanize_age(86400 * 2), "2일 전");
    }
}

// --- 창(window) 진행률 --------------------------------------------------------

/// 리셋 시각과 창 길이로 "시간이 얼마나 흘렀는지" 게이지를 만든다.
///
/// 남은 시간은 리셋 시각에서 역산한다. 창 길이를 모르면 만들 수 없다.
///
/// `now` 를 인자로 받는 이유: 내부에서 시계를 읽으면 호출 시점이 조금만 달라져도
/// 분이 내림되어 결과가 흔들리고, 테스트도 결정론적으로 쓸 수 없다.
pub fn time_bar(
    resets_at: chrono::DateTime<chrono::Local>,
    window: chrono::TimeDelta,
    now: chrono::DateTime<chrono::Local>,
) -> Option<Bar> {
    if window <= chrono::TimeDelta::zero() {
        return None;
    }
    let remaining = (resets_at - now).max(chrono::TimeDelta::zero());
    let elapsed = (window - remaining).max(chrono::TimeDelta::zero());
    let fill = elapsed.num_seconds() as f64 / window.num_seconds() as f64;

    Some(Bar {
        // remaining·elapsed 를 이미 0 이상으로 잘랐으므로 fill 은 0..=1 이다
        fill,
        label: time_left_label(remaining, window >= chrono::TimeDelta::days(1)),
        level: Level::Normal,
    })
}

/// `1 hour 12 minutes left` / `2 day 3 hour 15 minutes left`
///
/// 단위 표기를 단수로 고정한다. 자릿수가 일정해야 여러 줄이 세로로 맞는다.
fn time_left_label(remaining: chrono::TimeDelta, with_days: bool) -> String {
    let total = remaining.num_minutes().max(0);
    if with_days {
        let (d, h, m) = (total / 1440, (total % 1440) / 60, total % 60);
        format!("{d} day {h} hour {m} minutes left")
    } else {
        format!("{} hour {} minutes left", total / 60, total % 60)
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use chrono::{Local, TimeDelta};

    const SESSION: TimeDelta = TimeDelta::hours(5);
    const WEEK: TimeDelta = TimeDelta::days(7);

    /// 고정 기준 시각. 시계를 읽지 않으므로 결과가 흔들리지 않는다.
    fn now() -> chrono::DateTime<Local> {
        use chrono::TimeZone;
        Local.with_ymd_and_hms(2026, 8, 18, 21, 0, 0).single().unwrap()
    }

    #[test]
    fn half_elapsed_window_is_half_filled() {
        let bar = time_bar(now() + TimeDelta::minutes(150), SESSION, now()).unwrap();
        assert!((bar.fill - 0.5).abs() < 1e-9, "fill={}", bar.fill);
        assert_eq!(bar.label, "2 hour 30 minutes left");
    }

    /// 주간 창은 날짜까지 보여준다.
    #[test]
    fn weekly_label_includes_days() {
        let left = TimeDelta::days(2) + TimeDelta::hours(3) + TimeDelta::minutes(15);
        let bar = time_bar(now() + left, WEEK, now()).unwrap();
        assert_eq!(bar.label, "2 day 3 hour 15 minutes left");
        // 7일 중 2일 3시간 15분 남음 → 그만큼 지났다
        assert!((bar.fill - (1.0 - left.num_seconds() as f64 / WEEK.num_seconds() as f64)).abs() < 1e-9);
    }

    /// 이미 지난 리셋 시각이면 창이 다 찬 것으로 본다 — 음수 시간을 보여주면 안 된다.
    #[test]
    fn past_reset_is_full_and_zero() {
        let bar = time_bar(now() - TimeDelta::hours(1), SESSION, now()).unwrap();
        assert_eq!(bar.fill, 1.0);
        assert_eq!(bar.label, "0 hour 0 minutes left");
    }

    /// 리셋이 창 길이보다 멀리 있어도 비율이 음수가 되지 않는다.
    #[test]
    fn reset_beyond_window_clamps_to_zero() {
        let bar = time_bar(now() + TimeDelta::hours(10), SESSION, now()).unwrap();
        assert_eq!(bar.fill, 0.0);
    }

    #[test]
    fn zero_window_has_no_bar() {
        assert!(time_bar(now(), TimeDelta::zero(), now()).is_none());
    }
}
