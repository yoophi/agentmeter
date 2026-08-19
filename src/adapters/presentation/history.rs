//! 애플리케이션이 보관한 표본을 Sparkline 데이터로 투영한다.

use crate::application::UsageSample;
use crate::domain::usage::UsageWindow;

const MIN_POINTS: usize = 2;

pub(crate) fn chart(
    points: &[UsageSample],
    window: UsageWindow,
    width: usize,
) -> Option<Vec<Option<u64>>> {
    if width == 0 {
        return None;
    }
    let start = window.started_at().timestamp() / 60;
    let end = window.resets_at.timestamp() / 60;
    if end <= start {
        return None;
    }
    if points.len() < MIN_POINTS {
        return Some(vec![None; width]);
    }

    let span = (end - start) as f64;
    let mut cells = vec![None; width];
    for point in points {
        let offset = (point.minute - start) as f64 / span;
        if !(0.0..1.0).contains(&offset) {
            continue;
        }
        let index = ((offset * width as f64) as usize).min(width - 1);
        cells[index] = Some(point.percent.clamp(0.0, 100.0).round() as u64);
    }
    Some(cells)
}

pub(crate) fn delta(points: &[UsageSample]) -> Option<String> {
    if points.len() < MIN_POINTS {
        return None;
    }
    let difference = points.last()?.percent - points.first()?.percent;
    if difference.abs() < 0.5 {
        return None;
    }
    Some(format!("{difference:+.0}%p"))
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeDelta, TimeZone};

    use super::*;

    fn point(minute: i64, percent: f64) -> UsageSample {
        UsageSample { minute, percent }
    }

    fn at(minute: i64) -> chrono::DateTime<Local> {
        Local.timestamp_opt(minute * 60, 0).single().unwrap()
    }

    fn window(now_minute: i64, resets_in_minutes: i64) -> UsageWindow {
        UsageWindow {
            resets_at: at(now_minute + resets_in_minutes),
            duration: TimeDelta::hours(5),
        }
    }

    #[test]
    fn one_point_produces_an_empty_placeholder() {
        let output = chart(&[point(0, 10.0)], window(0, 60), 20).unwrap();
        assert_eq!(output, vec![None; 20]);
    }

    #[test]
    fn axis_spans_the_whole_window() {
        let points = [point(1000, 50.0), point(1001, 51.0), point(1002, 52.0)];
        let output = chart(&points, window(1002, 28), 20).unwrap();
        assert!(output[..15].iter().all(Option::is_none), "{output:?}");
    }

    #[test]
    fn delta_uses_first_and_last_samples() {
        let points = [point(0, 10.0), point(1, 17.0)];
        assert_eq!(delta(&points).as_deref(), Some("+7%p"));
        assert!(delta(&[point(0, 10.0), point(1, 10.1)]).is_none());
    }

    #[test]
    fn height_is_absolute_and_clamped() {
        let values = chart(
            &[point(0, -5.0), point(1, 50.0), point(2, 150.0)],
            window(2, 60),
            300,
        )
        .unwrap();
        assert!(values.contains(&Some(0)));
        assert!(values.contains(&Some(50)));
        assert!(values.contains(&Some(100)));
    }
}
