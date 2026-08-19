//! 한도 창별 사용률 변화 기록.
//!
//! 상주 모드에서 실측한 값을 5시간/7일 리셋 창별 파일에 저장한다. 앱을 다시
//! 실행해도 같은 창이면 이전 표본을 복원하고, 리셋 시각이 다르면 섞지 않는다.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::meter::{Meter, Window};

/// 차트가 의미를 갖는 최소 표본 수. 한 점만으로는 변화를 그릴 수 없다.
const MIN_POINTS: usize = 2;

/// 창 정보가 없는 메모리 전용 시리즈의 최대 표본 수.
const MAX_POINTS: usize = 512;

const FILE_VERSION: u8 = 1;

#[derive(Debug, Default)]
pub struct History {
    /// 창 정보가 없는 테스트·미래 한도를 위한 메모리 전용 표본.
    series: BTreeMap<String, Vec<Sample>>,
    /// 리셋 시각과 길이로 완전히 분리된 표본.
    windows: BTreeMap<WindowId, WindowSeries>,
    /// `~/.cache/agentmeter/history/<program>`; 없으면 메모리 전용.
    store_dir: Option<PathBuf>,
    /// 파일명에 들어가는 공급자 이름 (`claude` / `codex`).
    provider: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Sample {
    /// epoch 기준 분. 같은 분에 여러 번 조회하면 한 칸으로 합친다.
    minute: i64,
    percent: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct WindowId {
    duration_minutes: i64,
    resets_at_minute: i64,
}

impl WindowId {
    fn from_window(window: Window) -> Option<Self> {
        let duration_minutes = window.len.num_minutes();
        // 서버의 reset timestamp 는 같은 창에서도 59.8초/00.2초처럼 흔들릴 수 있다.
        // 절삭하면 서로 다른 분이 되므로 가장 가까운 분으로 정규화한다.
        let resets_at_minute = (window.resets_at.timestamp() + 30).div_euclid(60);
        (duration_minutes > 0).then_some(Self {
            duration_minutes,
            resets_at_minute,
        })
    }

    fn contains(self, minute: i64) -> bool {
        self.resets_at_minute - self.duration_minutes <= minute && minute < self.resets_at_minute
    }

    fn file_name(self, provider: &str) -> String {
        let duration = match self.duration_minutes {
            300 => "5H".to_string(),
            10_080 => "7D".to_string(),
            other => format!("{other}M"),
        };
        let start = minute_as_local(self.resets_at_minute - self.duration_minutes);
        let end = minute_as_local(self.resets_at_minute);
        format!(
            "{provider}__{duration}__{}__{}.json",
            start.format("%Y%m%d%H%M%S"),
            end.format("%Y%m%d%H%M%S")
        )
    }
}

#[derive(Debug, Default)]
struct WindowSeries {
    series: BTreeMap<String, Vec<Sample>>,
}

#[derive(Deserialize)]
struct HistoryFile {
    version: u8,
    window: WindowId,
    series: BTreeMap<String, Vec<Sample>>,
}

#[derive(Serialize)]
struct HistoryFileRef<'a> {
    version: u8,
    window: WindowId,
    series: &'a BTreeMap<String, Vec<Sample>>,
}

impl History {
    /// 실제 앱용 영속 히스토리. HOME 이 없으면 메모리 전용으로 동작한다.
    pub fn persistent(program: &str) -> Self {
        let Some(home) = std::env::var_os("HOME") else {
            return Self::default();
        };
        Self::persistent_at(
            program,
            PathBuf::from(home).join(".cache/agentmeter/history"),
        )
    }

    /// 파일시스템을 바꿔 끼우는 내부 seam. 테스트는 임시 디렉터리를 사용한다.
    fn persistent_at(program: &str, root: PathBuf) -> Self {
        let provider = match program {
            "ccmeter" => "claude",
            "codexmeter" => "codex",
            other => other.strip_suffix("meter").unwrap_or(other),
        };
        let safe_provider: String = provider
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        Self {
            store_dir: Some(root),
            provider: Some(safe_provider),
            ..Self::default()
        }
    }

    /// 한 번의 조회 결과를 기록한다.
    ///
    /// 같은 분에 다시 들어오면 **마지막 값으로 덮어쓴다.** 한 칸은 그 분이
    /// 끝났을 때의 상태를 뜻하는 편이 읽기 쉽다.
    pub fn record(&mut self, meters: &[Meter], at: DateTime<Local>) -> anyhow::Result<()> {
        let minute = at.timestamp() / 60;
        let mut dirty = BTreeSet::new();
        for m in meters {
            let percent = m.usage.fill_clamped() * 100.0;
            if let Some(id) = m.window.and_then(WindowId::from_window) {
                if !id.contains(minute) {
                    continue;
                }
                self.ensure_window_loaded(id)?;
                let points = self
                    .windows
                    .get_mut(&id)
                    .expect("방금 로드한 window")
                    .series
                    .entry(m.title.clone())
                    .or_default();
                record_sample(points, minute, percent, id.duration_minutes as usize);
                dirty.insert(id);
            } else {
                let points = self.series.entry(m.title.clone()).or_default();
                record_sample(points, minute, percent, MAX_POINTS);
            }
        }
        for id in dirty {
            self.persist_window(id)?;
        }
        Ok(())
    }

    /// 한 한도의 변화를 창 전체 기간에 걸친 sparkline 데이터로 만든다.
    ///
    /// 가로축은 **창 전체**(세션 5시간 / 주간 7일)다. 바로 위의 시간 게이지와
    /// 같은 축이라 나란히 놓으면 "창의 어느 지점에서 얼마나 썼는지" 가 읽힌다.
    /// 아직 수집하지 못한 구간은 데이터가 없으므로 `·` 로 비워 둔다.
    ///
    /// 세로축은 0~100% 고정이다. `None` 은 아직 수집하지 못했거나
    /// 조회가 없던 구간이며 렌더러가 따로 표시한다.
    pub fn chart(&self, title: &str, window: Window, width: usize) -> Option<Vec<Option<u64>>> {
        if width == 0 {
            return None;
        }
        let id = WindowId::from_window(window)?;
        let start = id.resets_at_minute - id.duration_minutes;
        let end = id.resets_at_minute;
        if end <= start {
            return None;
        }

        let points = self.points(title, Some(window));
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

        Some(
            cells
                .into_iter()
                .map(|cell| cell.map(sparkline_value))
                .collect(),
        )
    }

    /// 현재 한도 창에서 기록된 첫 표본 이후 얼마나 늘었는지 — `+6%p`.
    ///
    /// 차트 옆에 붙이면 폭이 밀리므로 제목 줄에 따로 놓는다.
    /// 변화가 없으면 `None` — 굳이 `0%p` 를 띄울 이유가 없다.
    pub fn delta(&self, title: &str, window: Option<Window>) -> Option<String> {
        let bounds = window.and_then(WindowId::from_window).map(|id| {
            (
                id.resets_at_minute - id.duration_minutes,
                id.resets_at_minute,
            )
        });
        let mut points = self.points(title, window).iter().filter(|point| {
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

    fn points(&self, title: &str, window: Option<Window>) -> &[Sample] {
        window
            .and_then(WindowId::from_window)
            .and_then(|id| self.windows.get(&id))
            .and_then(|history| history.series.get(title))
            // 창 없는 기존 호출도 메모리 전용 표본을 사용할 수 있게 한다.
            .or_else(|| self.series.get(title))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn ensure_window_loaded(&mut self, id: WindowId) -> anyhow::Result<()> {
        if self.windows.contains_key(&id) {
            return Ok(());
        }
        let history = match &self.store_dir {
            Some(dir) => read_window_file(&dir.join(id.file_name(self.provider_name()?)), id)?,
            None => WindowSeries::default(),
        };
        self.windows.insert(id, history);
        Ok(())
    }

    fn persist_window(&self, id: WindowId) -> anyhow::Result<()> {
        let Some(dir) = &self.store_dir else {
            return Ok(());
        };
        let history = self
            .windows
            .get(&id)
            .context("저장할 window 를 찾을 수 없습니다")?;
        write_window_file(&dir.join(id.file_name(self.provider_name()?)), id, history)
    }

    fn provider_name(&self) -> anyhow::Result<&str> {
        self.provider
            .as_deref()
            .context("히스토리 공급자 이름이 없습니다")
    }
}

fn minute_as_local(minute: i64) -> DateTime<Local> {
    Utc.timestamp_opt(minute * 60, 0)
        .single()
        .expect("Window 에서 얻은 유효한 timestamp")
        .with_timezone(&Local)
}

fn record_sample(points: &mut Vec<Sample>, minute: i64, percent: f64, max_points: usize) {
    match points.binary_search_by_key(&minute, |sample| sample.minute) {
        Ok(index) => points[index].percent = percent,
        Err(index) => points.insert(index, Sample { minute, percent }),
    }
    if points.len() > max_points {
        points.drain(..points.len() - max_points);
    }
}

fn read_window_file(path: &Path, expected: WindowId) -> anyhow::Result<WindowSeries> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(WindowSeries::default()),
        Err(e) => return Err(e).context("히스토리 파일을 읽지 못했습니다"),
    };
    let file: HistoryFile = serde_json::from_str(&raw).context("히스토리 파일 파싱 실패")?;
    anyhow::ensure!(file.version == FILE_VERSION, "지원하지 않는 히스토리 버전");
    anyhow::ensure!(
        file.window == expected,
        "히스토리 window 가 파일명과 다릅니다"
    );
    Ok(WindowSeries {
        series: file.series,
    })
}

fn write_window_file(path: &Path, id: WindowId, history: &WindowSeries) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .context("히스토리 디렉터리를 찾을 수 없습니다")?;
    std::fs::create_dir_all(dir).context("히스토리 디렉터리를 만들지 못했습니다")?;
    let file = HistoryFileRef {
        version: FILE_VERSION,
        window: id,
        series: &history.series,
    };
    let bytes = serde_json::to_vec(&file).context("히스토리 직렬화 실패")?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).context("히스토리 임시 파일 저장 실패")?;
    std::fs::rename(&tmp, path).context("히스토리 파일 교체 실패")?;
    Ok(())
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

    fn windowed_meter(title: &str, percent: f64, window: Window) -> Meter {
        let mut m = meter(title, percent);
        m.window = Some(window);
        m
    }

    #[test]
    fn reset_timestamp_jitter_maps_to_the_same_window() {
        let boundary = at(20_000);
        let before = Window {
            resets_at: boundary - TimeDelta::milliseconds(200),
            len: TimeDelta::hours(5),
        };
        let after = Window {
            resets_at: boundary + TimeDelta::milliseconds(200),
            len: TimeDelta::hours(5),
        };

        assert_eq!(WindowId::from_window(before), WindowId::from_window(after));
        assert_eq!(
            WindowId::from_window(before).unwrap().resets_at_minute,
            20_000
        );
    }

    fn temp_history_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("agentmeter-history-{}-{label}", std::process::id()))
    }

    #[test]
    fn shows_an_empty_placeholder_until_two_points_exist() {
        let mut h = History::default();
        let empty = vec![None; 20];

        assert_eq!(h.chart("A", window(0, 60), 20), Some(empty.clone()));
        h.record(&[meter("A", 10.0)], at(0)).unwrap();
        assert_eq!(h.chart("A", window(0, 60), 20), Some(empty));

        h.record(&[meter("A", 12.0)], at(1)).unwrap();
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
            h.record(&[meter("A", p)], at(5)).unwrap();
        }
        assert_eq!(
            h.chart("A", window(5, 60), 20),
            Some(vec![None; 20]),
            "아직 한 표본이므로 placeholder 유지"
        );
        h.record(&[meter("A", 20.0)], at(6)).unwrap();
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
            h.record(&[meter("A", 50.0)], at(now + i)).unwrap();
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
        h.record(&[meter("A", 90.0)], at(0)).unwrap(); // 아주 오래된 표본
        h.record(&[meter("A", 10.0)], at(1000)).unwrap();
        h.record(&[meter("A", 12.0)], at(1001)).unwrap();
        let chart = h.chart("A", window(1001, 30), 20).unwrap();
        assert!(
            !chart.contains(&Some(90)),
            "창 밖 90% 가 그려지면 안 됨: {chart:?}"
        );
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
            h.record(&[meter("A", a), meter("B", b)], at(1000 + i as i64))
                .unwrap();
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
        h.record(&[meter("A", 30.0)], at(1000)).unwrap();
        h.record(&[meter("A", 30.0)], at(1001)).unwrap();
        assert!(h.delta("A", None).is_none());
    }

    /// 줄어든 경우(창 리셋 등)도 부호와 함께 보여준다.
    #[test]
    fn negative_delta_keeps_its_sign() {
        let mut h = History::default();
        h.record(&[meter("A", 80.0)], at(1000)).unwrap();
        h.record(&[meter("A", 5.0)], at(1001)).unwrap();
        assert_eq!(h.delta("A", None).unwrap(), "-75%p");
    }

    /// 창이 리셋되면 이전 창의 높은 사용률을 변화량에 섞지 않는다.
    #[test]
    fn delta_uses_only_samples_from_the_current_window() {
        let mut h = History::default();
        h.record(&[meter("A", 70.0)], at(299)).unwrap(); // 이전 창
        h.record(&[meter("A", 2.0)], at(300)).unwrap(); // 새 창 시작
        h.record(&[meter("A", 4.0)], at(301)).unwrap();

        let current = window(301, 299); // 300..600
        assert_eq!(h.delta("A", Some(current)).as_deref(), Some("+2%p"));
    }

    #[test]
    fn zero_width_has_no_chart() {
        let mut h = History::default();
        h.record(&[meter("A", 10.0)], at(1000)).unwrap();
        h.record(&[meter("A", 11.0)], at(1001)).unwrap();
        assert!(h.chart("A", window(1001, 30), 0).is_none());
    }

    /// 오래 돌아도 표본이 무한히 쌓이지 않는다.
    #[test]
    fn caps_the_number_of_samples() {
        let mut h = History::default();
        for i in 0..(MAX_POINTS as i64 + 20) {
            h.record(&[meter("A", i as f64 % 100.0)], at(i)).unwrap();
        }
        assert_eq!(h.series["A"].len(), MAX_POINTS);
    }

    #[test]
    fn restores_samples_when_restart_uses_the_same_window() {
        let dir = temp_history_dir("restore-same-window");
        let _ = std::fs::remove_dir_all(&dir);
        let w = window(1000, 200); // 800..1200

        let mut first = History::persistent_at("ccmeter", dir.clone());
        first
            .record(&[windowed_meter("A", 10.0, w)], at(1000))
            .unwrap();
        first
            .record(&[windowed_meter("A", 20.0, w)], at(1001))
            .unwrap();
        drop(first);

        let mut restarted = History::persistent_at("ccmeter", dir.clone());
        restarted
            .record(&[windowed_meter("A", 25.0, w)], at(1002))
            .unwrap();
        assert_eq!(restarted.delta("A", Some(w)).as_deref(), Some("+15%p"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_reset_window_does_not_load_the_previous_window() {
        let dir = temp_history_dir("isolate-reset-window");
        let _ = std::fs::remove_dir_all(&dir);
        let old = window(1000, 100); // 800..1100
        let new = window(1100, 300); // 1100..1400

        let mut first = History::persistent_at("ccmeter", dir.clone());
        first
            .record(&[windowed_meter("A", 90.0, old)], at(1000))
            .unwrap();
        drop(first);

        let mut restarted = History::persistent_at("ccmeter", dir.clone());
        restarted
            .record(&[windowed_meter("A", 2.0, new)], at(1100))
            .unwrap();
        restarted
            .record(&[windowed_meter("A", 4.0, new)], at(1101))
            .unwrap();
        assert_eq!(restarted.delta("A", Some(new)).as_deref(), Some("+2%p"));

        let files = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(files, 2, "각 리셋 창은 별도 파일이어야 함");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn five_hour_and_seven_day_windows_use_unique_files() {
        let dir = temp_history_dir("duration-file-names");
        let _ = std::fs::remove_dir_all(&dir);
        let reset = at(20_000);
        let five_hour = Window {
            resets_at: reset,
            len: TimeDelta::hours(5),
        };
        let seven_day = Window {
            resets_at: reset,
            len: TimeDelta::days(7),
        };
        let measured_at = at(19_900);

        let mut claude = History::persistent_at("ccmeter", dir.clone());
        claude
            .record(
                &[windowed_meter("Current session", 11.0, five_hour)],
                measured_at,
            )
            .unwrap();
        let mut codex = History::persistent_at("codexmeter", dir.clone());
        codex
            .record(
                &[windowed_meter("Current week (all models)", 22.0, seven_day)],
                measured_at,
            )
            .unwrap();

        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names.len(), 2);
        assert!(
            names.iter().any(|name| name.starts_with("claude__5H__")),
            "{names:?}"
        );
        assert!(
            names.iter().any(|name| name.starts_with("codex__7D__")),
            "{names:?}"
        );
        assert!(
            names.iter().all(|name| !name.contains(['(', ')'])),
            "{names:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
