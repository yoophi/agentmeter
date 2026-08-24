//! 애플리케이션이 보관한 표본을 Sparkline 데이터로 투영한다.

use crate::application::UsageSample;
use crate::domain::usage::UsageWindow;

const MIN_POINTS: usize = 2;

/// 정상 조회 간격의 몇 배부터 "기록이 끊겼다"고 볼지.
const GAP_FACTOR: i64 = 4;

/// 이보다 짧은 결측은 공백으로 보지 않는다.
///
/// 조회 지연이나 짧은 재시작으로 표본 몇 개가 빠지는 일은 흔하다. 그것까지
/// 끊으면 차트가 잘게 조각나 정작 긴 공백이 눈에 띄지 않는다.
const MIN_GAP_MINUTES: i64 = 10;

/// 기록이 끊긴 자리에서 표본을 끊어 구간별로 나눈다.
///
/// 조회 주기는 설정(`--interval`)에 따라 다르므로 고정 임계값만으로는 부족하다.
/// 표본 간격의 중앙값을 실제 주기로 보고 그 몇 배를 넘으면 공백으로 판정하되,
/// 하한을 둬서 짧은 결측은 끊지 않는다. 간격이 하나뿐이면 주기를 알 수 없으니
/// 나누지 않는다.
pub(crate) fn segments(points: &[UsageSample]) -> Vec<&[UsageSample]> {
    if points.is_empty() {
        return Vec::new();
    }
    let Some(limit) = gap_limit(points) else {
        return vec![points];
    };

    let mut runs = Vec::new();
    let mut start = 0;
    for index in 1..points.len() {
        if points[index].minute - points[index - 1].minute > limit {
            runs.push(&points[start..index]);
            start = index;
        }
    }
    runs.push(&points[start..]);
    runs
}

fn gap_limit(points: &[UsageSample]) -> Option<i64> {
    let mut gaps: Vec<i64> = points
        .windows(2)
        .map(|pair| pair[1].minute - pair[0].minute)
        .collect();
    if gaps.len() < MIN_POINTS {
        return None;
    }
    gaps.sort_unstable();
    let median = gaps[gaps.len() / 2];
    // 촘촘한 구간이 많으면 중앙값이 그쪽으로 끌려가므로 하한이 필요하다.
    Some((median * GAP_FACTOR).max(MIN_GAP_MINUTES))
}

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
    fn uniform_samples_stay_one_segment() {
        let points: Vec<UsageSample> = (0..10).map(|minute| point(minute, 10.0)).collect();
        let runs = segments(&points);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len(), 10);
    }

    #[test]
    fn a_long_pause_splits_the_series() {
        let mut points: Vec<UsageSample> = (0..5).map(|minute| point(minute, 10.0)).collect();
        // 조회가 3시간 멈춘 뒤 다시 기록된 구간.
        points.extend((180..185).map(|minute| point(minute, 40.0)));
        let runs = segments(&points);
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert_eq!(runs[0].len(), 5);
        assert_eq!(runs[1].len(), 5);
    }

    #[test]
    fn a_single_missed_sample_is_not_a_gap() {
        // 1분 주기에서 한 번 빠진 것은 조회가 멈춘 것이 아니다.
        let points = [
            point(0, 10.0),
            point(1, 11.0),
            point(3, 12.0),
            point(4, 13.0),
        ];
        assert_eq!(segments(&points).len(), 1);
    }

    #[test]
    fn a_longer_interval_scales_the_threshold() {
        // 5분 주기로 기록하면 5분 간격은 공백이 아니다.
        let points: Vec<UsageSample> = (0..5).map(|step| point(step * 5, 10.0)).collect();
        assert_eq!(segments(&points).len(), 1);
    }

    #[test]
    fn dense_early_samples_do_not_turn_a_slower_tail_into_gaps() {
        // 앞은 1분 간격 20개, 2시간 쉬고, 뒤는 4분 간격 9개.
        let mut points: Vec<UsageSample> = (0..20).map(|minute| point(minute, 10.0)).collect();
        points.extend((0..9).map(|step| point(140 + step * 4, 40.0)));
        let runs = segments(&points);
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert_eq!(
            runs[1].len(),
            9,
            "뒤쪽 4분 간격은 조회 주기의 흔들림이지 공백이 아니다"
        );
    }

    #[test]
    fn two_samples_cannot_reveal_a_period_so_they_stay_joined() {
        let points = [point(0, 10.0), point(500, 90.0)];
        assert_eq!(segments(&points).len(), 1);
    }

    #[test]
    fn no_samples_produce_no_segments() {
        assert!(segments(&[]).is_empty());
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
