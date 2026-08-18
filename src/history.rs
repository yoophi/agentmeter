//! 프로세스가 사는 동안 모은 사용률 변화.
//!
//! 상주 모드에서만 의미가 있다 — 1회 실행은 표본이 하나뿐이다.
//! 디스크에 남기지 않으므로 "앱을 켠 뒤로" 무엇이 얼마나 늘었는지만 보여준다.

use std::collections::BTreeMap;

use chrono::{DateTime, Local};

use crate::meter::{Meter, Window};

/// 차트가 의미를 갖는 최소 표본 수. 한 점만으로는 변화를 그릴 수 없다.
const MIN_POINTS: usize = 2;

/// 보관할 최대 표본 수. 주간 창(7일)을 분 단위로 다 담을 수는 없으므로
/// 오래된 쪽부터 버린다. 차트 폭보다 넉넉하면 충분하다.
const MAX_POINTS: usize = 512;

#[derive(Debug, Default)]
pub struct History {
    /// 한도 제목 → 분 단위 표본. 제목이 그 한도의 정체성이다.
    series: BTreeMap<String, Vec<Sample>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    /// epoch 기준 분. 같은 분에 여러 번 조회하면 한 칸으로 합친다.
    minute: i64,
    percent: f64,
}

impl History {
    /// 한 번의 조회 결과를 기록한다.
    ///
    /// 같은 분에 다시 들어오면 **마지막 값으로 덮어쓴다.** 한 칸은 그 분이
    /// 끝났을 때의 상태를 뜻하는 편이 읽기 쉽다.
    pub fn record(&mut self, meters: &[Meter], at: DateTime<Local>) {
        let minute = at.timestamp() / 60;
        for m in meters {
            let points = self.series.entry(m.title.clone()).or_default();
            let percent = m.usage.fill_clamped() * 100.0;
            match points.last_mut() {
                Some(last) if last.minute == minute => last.percent = percent,
                _ => points.push(Sample { minute, percent }),
            }
            if points.len() > MAX_POINTS {
                points.remove(0);
            }
        }
    }

    /// 한 한도의 변화를 창 전체 기간에 걸친 sparkline 데이터로 만든다.
    ///
    /// 가로축은 **창 전체**(세션 5시간 / 주간 7일)다. 바로 위의 시간 게이지와
    /// 같은 축이라 나란히 놓으면 "창의 어느 지점에서 얼마나 썼는지" 가 읽힌다.
    /// 앱을 켜기 전 구간은 데이터가 없으므로 `·` 로 비워 둔다.
    ///
    /// 세로축은 0~100% 고정이다. `None` 은 앱을 켜기 전이거나
    /// 조회가 없던 구간이며 렌더러가 따로 표시한다.
    pub fn chart(&self, title: &str, window: Window, width: usize) -> Option<Vec<Option<u64>>> {
        if width == 0 {
            return None;
        }
        let start = window.started_at().timestamp() / 60;
        let end = window.resets_at.timestamp() / 60;
        if end <= start {
            return None;
        }

        let points = self.series.get(title).map(Vec::as_slice).unwrap_or_default();
        if points.len() < MIN_POINTS {
            // 한 점만으로는 변화를 그릴 수 없다. 차트를 숨기면 첫 조회 후
            // 레이아웃이 튀므로, 같은 크기의 빈 placeholder 를 보여준다.
            return Some(vec![None; width]);
        }

        // 각 칸이 담당하는 시간 구간에 표본이 있으면 그 값을, 없으면 빈칸을 그린다
        let span = (end - start) as f64;
        let mut cells: Vec<Option<f64>> = vec![None; width];
        for p in points {
            let offset = (p.minute - start) as f64 / span;
            if !(0.0..1.0).contains(&offset) {
                continue; // 창 밖의 표본 — 창이 리셋되기 전/후의 것
            }
            let idx = ((offset * width as f64) as usize).min(width - 1);
            // 같은 칸에 여러 표본이 들어오면 마지막 값이 그 칸의 상태다
            cells[idx] = Some(p.percent);
        }

        Some(cells.into_iter().map(|cell| cell.map(sparkline_value)).collect())
    }

    /// 앱을 켠 뒤로 이 한도가 얼마나 늘었는지 — `+6%p`.
    ///
    /// 차트 옆에 붙이면 폭이 밀리므로 제목 줄에 따로 놓는다.
    /// 변화가 없으면 `None` — 굳이 `0%p` 를 띄울 이유가 없다.
    pub fn delta(&self, title: &str, window: Option<Window>) -> Option<String> {
        let bounds = window.map(|w| {
            (
                w.started_at().timestamp() / 60,
                w.resets_at.timestamp() / 60,
            )
        });
        let mut points = self.series.get(title)?.iter().filter(|point| {
            bounds.is_none_or(|(start, end)| start <= point.minute && point.minute < end)
        });
        let first = points.next()?;
        let second = points.next()?;
        let last = points.next_back().unwrap_or(second);
        let diff = last.percent - first.percent;
        if diff.abs() < 0.5 {
            return None;
        }
        Some(format!("{diff:+.0}%p"))
    }
}

/// Ratatui Sparkline 에 넘길 0~100 정수값.
fn sparkline_value(percent: f64) -> u64 {
    percent.clamp(0.0, 100.0).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meter::{Bar, Level};
    use chrono::{TimeDelta, TimeZone};

    fn meter(title: &str, percent: f64) -> Meter {
        Meter {
            title: title.into(),
            usage: Bar::used(percent, Some(Level::Normal)),
            window: None,
            time: None,
            footnote: None,
            emphasized: false,
        }
    }

    fn at(minute: i64) -> DateTime<Local> {
        Local.timestamp_opt(minute * 60, 0).single().unwrap()
    }

    /// 5시간 창. `resets_in_min` 분 뒤에 리셋된다.
    fn window(now_min: i64, resets_in_min: i64) -> Window {
        Window {
            resets_at: at(now_min + resets_in_min),
            len: TimeDelta::hours(5),
        }
    }

    #[test]
    fn shows_an_empty_placeholder_until_two_points_exist() {
        let mut h = History::default();
        let empty = vec![None; 20];

        assert_eq!(h.chart("A", window(0, 60), 20), Some(empty.clone()));
        h.record(&[meter("A", 10.0)], at(0));
        assert_eq!(h.chart("A", window(0, 60), 20), Some(empty));

        h.record(&[meter("A", 12.0)], at(1));
        assert!(
            h.chart("A", window(1, 60), 20)
                .unwrap()
                .iter()
                .any(Option::is_some),
            "두 표본부터는 실제 값이 보여야 함"
        );
    }

    /// 같은 분에 여러 번 조회해도 칸은 하나이고, 마지막 값이 남는다.
    #[test]
    fn same_minute_collapses_to_one_point() {
        let mut h = History::default();
        for p in [10.0, 11.0, 13.0] {
            h.record(&[meter("A", p)], at(5));
        }
        assert_eq!(
            h.chart("A", window(5, 60), 20),
            Some(vec![None; 20]),
            "아직 한 표본이므로 placeholder 유지"
        );
        h.record(&[meter("A", 20.0)], at(6));
        assert_eq!(h.chart("A", window(6, 60), 20).unwrap().len(), 20);
        assert_eq!(h.delta("A", None).unwrap(), "+7%p");
    }

    /// 가로축은 창 전체. 앱을 켜기 전 구간은 비어 있어야 한다.
    #[test]
    fn axis_spans_the_whole_window_and_leaves_the_past_empty() {
        let mut h = History::default();
        // 창은 300분. 지금은 창의 90% 지점(리셋까지 30분)에서 시작했다.
        let now = 1000;
        for i in 0..3 {
            h.record(&[meter("A", 50.0)], at(now + i));
        }
        let chart = h.chart("A", window(now + 2, 28), 20).unwrap();
        // 앞쪽 대부분은 데이터가 없으므로 빈칸이어야 한다
        assert!(
            chart.iter().take(15).all(Option::is_none),
            "앞 구간이 비어야 함: {chart:?}"
        );
        assert!(chart.contains(&Some(50)), "50% 칸이 있어야 함: {chart:?}");
    }

    /// 세로축은 0~100% 고정 — 절대값이 그대로 높이가 된다.
    #[test]
    fn height_is_absolute_percent() {
        assert_eq!(sparkline_value(0.0), 0);
        assert_eq!(sparkline_value(100.0), 100);
        assert_eq!(sparkline_value(50.4), 50);
        assert_eq!(sparkline_value(50.5), 51);
        // 범위를 벗어나도 잘라낸다
        assert_eq!(sparkline_value(-5.0), 0);
        assert_eq!(sparkline_value(150.0), 100);
    }

    /// 창 밖(이전 창)의 표본은 그리지 않는다.
    #[test]
    fn samples_outside_the_window_are_ignored() {
        let mut h = History::default();
        h.record(&[meter("A", 90.0)], at(0)); // 아주 오래된 표본
        h.record(&[meter("A", 10.0)], at(1000));
        h.record(&[meter("A", 12.0)], at(1001));
        let chart = h.chart("A", window(1001, 30), 20).unwrap();
        assert!(!chart.contains(&Some(90)), "창 밖 90% 가 그려지면 안 됨: {chart:?}");
    }

    #[test]
    fn unknown_title_gets_an_empty_placeholder() {
        let h = History::default();
        assert_eq!(h.chart("없음", window(0, 60), 20), Some(vec![None; 20]));
    }

    /// 여러 한도를 따로 추적한다.
    #[test]
    fn tracks_each_limit_separately() {
        let mut h = History::default();
        for (i, (a, b)) in [(10.0, 50.0), (20.0, 51.0)].into_iter().enumerate() {
            h.record(&[meter("A", a), meter("B", b)], at(1000 + i as i64));
        }
        let w = window(1001, 30);
        assert!(h.chart("A", w, 20).is_some());
        assert_eq!(h.delta("A", None).unwrap(), "+10%p");
        assert_eq!(h.delta("B", None).unwrap(), "+1%p");
    }

    /// 줄지 않았으면 굳이 표시하지 않는다.
    #[test]
    fn no_delta_when_unchanged() {
        let mut h = History::default();
        h.record(&[meter("A", 30.0)], at(1000));
        h.record(&[meter("A", 30.0)], at(1001));
        assert!(h.delta("A", None).is_none());
    }

    /// 줄어든 경우(창 리셋 등)도 부호와 함께 보여준다.
    #[test]
    fn negative_delta_keeps_its_sign() {
        let mut h = History::default();
        h.record(&[meter("A", 80.0)], at(1000));
        h.record(&[meter("A", 5.0)], at(1001));
        assert_eq!(h.delta("A", None).unwrap(), "-75%p");
    }

    /// 창이 리셋되면 이전 창의 높은 사용률을 변화량에 섞지 않는다.
    #[test]
    fn delta_uses_only_samples_from_the_current_window() {
        let mut h = History::default();
        h.record(&[meter("A", 70.0)], at(299)); // 이전 창
        h.record(&[meter("A", 2.0)], at(300)); // 새 창 시작
        h.record(&[meter("A", 4.0)], at(301));

        let current = window(301, 299); // 300..600
        assert_eq!(h.delta("A", Some(current)).as_deref(), Some("+2%p"));
    }

    #[test]
    fn zero_width_has_no_chart() {
        let mut h = History::default();
        h.record(&[meter("A", 10.0)], at(1000));
        h.record(&[meter("A", 11.0)], at(1001));
        assert!(h.chart("A", window(1001, 30), 0).is_none());
    }

    /// 오래 돌아도 표본이 무한히 쌓이지 않는다.
    #[test]
    fn caps_the_number_of_samples() {
        let mut h = History::default();
        for i in 0..(MAX_POINTS as i64 + 20) {
            h.record(&[meter("A", i as f64 % 100.0)], at(i));
        }
        assert_eq!(h.series["A"].len(), MAX_POINTS);
    }
}
