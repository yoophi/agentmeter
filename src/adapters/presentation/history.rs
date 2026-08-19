//! 애플리케이션이 보관한 표본을 텍스트 차트로 투영한다.

use crate::application::UsageSample;
use crate::domain::usage::UsageWindow;

const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const EMPTY: char = '·';
const MIN_POINTS: usize = 2;

pub(crate) fn chart(points: &[UsageSample], window: UsageWindow, width: usize) -> Option<String> {
    if points.len() < MIN_POINTS || width == 0 {
        return None;
    }
    let start = window.started_at().timestamp() / 60;
    let end = window.resets_at.timestamp() / 60;
    if end <= start {
        return None;
    }

    let span = (end - start) as f64;
    let mut cells: Vec<Option<f64>> = vec![None; width];
    for point in points {
        let offset = (point.minute - start) as f64 / span;
        if !(0.0..1.0).contains(&offset) {
            continue;
        }
        let index = ((offset * width as f64) as usize).min(width - 1);
        cells[index] = Some(point.percent);
    }

    Some(cells.iter().map(|cell| cell.map_or(EMPTY, block)).collect())
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

fn block(percent: f64) -> char {
    let ratio = (percent / 100.0).clamp(0.0, 1.0);
    let index = (ratio * (BLOCKS.len() - 1) as f64).round() as usize;
    BLOCKS[index.min(BLOCKS.len() - 1)]
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
    fn chart_needs_two_points() {
        assert!(chart(&[point(0, 10.0)], window(0, 60), 20).is_none());
        assert!(chart(&[point(0, 10.0), point(1, 12.0)], window(1, 60), 20).is_some());
    }

    #[test]
    fn axis_spans_the_whole_window() {
        let points = [point(1000, 50.0), point(1001, 51.0), point(1002, 52.0)];
        let output = chart(&points, window(1002, 28), 20).unwrap();
        assert!(
            output.starts_with(&EMPTY.to_string().repeat(15)),
            "{output}"
        );
    }

    #[test]
    fn delta_uses_first_and_last_samples() {
        let points = [point(0, 10.0), point(1, 17.0)];
        assert_eq!(delta(&points).as_deref(), Some("+7%p"));
        assert!(delta(&[point(0, 10.0), point(1, 10.1)]).is_none());
    }

    #[test]
    fn block_height_is_absolute_and_clamped() {
        assert_eq!(block(-5.0), '▁');
        assert_eq!(block(50.0), '▅');
        assert_eq!(block(150.0), '█');
    }
}
