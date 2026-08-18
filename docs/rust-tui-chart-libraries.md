# Rust TUI ASCII/Unicode 차트 라이브러리 조사

- 조회일: 2026-08-19 (Asia/Seoul)
- 대상 프로젝트: `agentmeter` (`ratatui 0.30.2`, `crossterm 0.29.0`)
- 조사 범위: 공식 GitHub 저장소/API, crates.io API, docs.rs 및 공식 프로젝트 문서만 사용

## 결론

> 구현 결정: 조사 후 내장 `Sparkline`을 채택했다. `agentmeter`는 한 화면에
> 최대 3개의 한도를 보여주므로, 4~6행 일반 권고안보다 보수적인 3행을
> 기본값으로 선택했다.

이 프로젝트에는 **Ratatui 내장 `Sparkline`을 4~6행으로 확장하는 방법**을 우선
추천한다. 이미 쓰는 `ratatui 0.30.2` 안에 있고, `Option<u64>`로 빈 시간 구간을
표현하며, 0~100% 고정 축을 유지할 수 있다. 현재 1행 8단계 문자보다 높이 해상도가
크게 좋아지면서 신규 의존성이 없다.

변화량의 기울기를 막대 면적보다 **선**으로 읽는 것이 중요하고 X/Y축 또는 시간
레이블까지 넣을 계획이라면 내장 `Chart`와 `Marker::Braille`이 두 번째 선택이다.
외부 크레이트 중에는 `tui-bar-graph`가 유일하게 현재 Ratatui 버전과 자연스럽게
통합된다. 나머지 문자열/CLI 차트는 Ratatui의 `Buffer`, `Style`, 레이아웃과 별개라
이 프로젝트에서는 얻는 것보다 어댑터 비용이 크다.

추천 순위는 다음과 같다.

1. `ratatui::widgets::Sparkline`, 높이 4~6행
2. `ratatui::widgets::Chart` + `GraphType::Line` + `Marker::Braille`
3. 시각적 그라데이션이 제품 요구사항일 때만 `tui-bar-graph`

## 현재 구현과 평가 기준

현재 `History::chart()`는 한도 창 전체를 가로축, 사용률 0~100%를 세로축으로 하여
각 열을 `·` 또는 `▁`~`█` 문자 하나로 만든다. TUI는 이를 `Paragraph` 한 행으로
렌더링한다. 후보는 다음 조건으로 평가했다.

- 3~6행 안에서 사용률 변화가 1행보다 명확해야 한다.
- 5시간/7일 창 전체에서 아직 측정하지 않은 구간을 비워 둘 수 있어야 한다.
- 0~100% 고정 범위와 터미널 폭 변경을 처리할 수 있어야 한다.
- Ratatui의 `Frame`/`Buffer`, 색상, 테스트 백엔드와 잘 결합되어야 한다.
- 이미 쓰는 `ratatui 0.30.2`와 동일한 타입 계열이어야 한다.

## 인기도와 유지보수 현황

별과 fork는 2026-08-19에 GitHub REST API의 `stargazers_count`와 `forks_count`를
조회한 스냅샷이다. `tui-bar-graph`는 모노레포 안의 크레이트이므로 저장소 전체
수치이며, Ratatui 내장 위젯 둘은 같은 저장소 수치를 공유한다. 다운로드 수는
품질 점수가 아니라 채택 정도를 보조하는 crates.io 누적 수치다.

| 후보 | GitHub stars / forks | 최신 crate / 배포일 | 누적 다운로드 | 최근 저장소 활동 | 유지보수 판단 |
|---|---:|---|---:|---|---|
| Ratatui 내장 `Sparkline`, `Chart` | 22,269 / 743 | 0.30.2 / 2026-06-19 | 44,516,488 | 최근 commit 2026-08-16, push 2026-08-17 | 매우 활발. 프로젝트가 이미 정확히 0.30.2 사용 |
| `tui-bar-graph` | 227 / 25 | 0.3.5 / 2026-06-14 | 42,857 | 모노레포 최근 commit 2026-08-02, push 2026-08-12 | 활발. 0.3.1~0.3.5가 약 6개월 사이 배포됨 |
| `textplots` | 286 / 29 | 0.8.7 / 2025-02-20 | 1,113,618 | 최근 commit/push 2025-02-20 | 채택은 많지만 18개월간 코드 활동 없음 |
| `asciigraph-rs` | 140 / 6 | 0.1.5 / 2026-05-03 | 280 | 최근 push 2026-05-14 | 최근 활동은 있으나 매우 젊고 채택 표본이 작음 |
| `rasciigraph` | 102 / 4 | 0.3.0 / 2025-06-09 | 98,669 | 최근 commit/push 2025-06-09 | 작고 안정적이나 14개월간 활동 없음 |
| `plotters-ratatui-backend` | 11 / 3 | 0.3.0 / 2025-04-05 | 13,565 | 최근 commit/push 2025-04-05 | 활동이 뜸하고 현재 프로젝트 버전과 불일치 |
| `rasciichart` | 1 / 0 | 0.2.17 / 2025-12-15 | 2,434 | 최근 commit/push 2025-12-15 | 채택률이 매우 낮음 |

수치 출처:

- GitHub API: [ratatui/ratatui](https://api.github.com/repos/ratatui/ratatui), [ratatui/tui-widgets](https://api.github.com/repos/ratatui/tui-widgets), [loony-bean/textplots-rs](https://api.github.com/repos/loony-bean/textplots-rs), [neneodonkor/asciigraph-rs](https://api.github.com/repos/neneodonkor/asciigraph-rs), [orhanbalci/rasciigraph](https://api.github.com/repos/orhanbalci/rasciigraph), [SOF3/plotters-ratatui-backend](https://api.github.com/repos/SOF3/plotters-ratatui-backend), [cumulus13/rasciichart](https://api.github.com/repos/cumulus13/rasciichart)
- crates.io API: [ratatui](https://crates.io/api/v1/crates/ratatui), [tui-bar-graph](https://crates.io/api/v1/crates/tui-bar-graph), [textplots](https://crates.io/api/v1/crates/textplots), [asciigraph-rs](https://crates.io/api/v1/crates/asciigraph-rs), [rasciigraph](https://crates.io/api/v1/crates/rasciigraph), [plotters-ratatui-backend](https://crates.io/api/v1/crates/plotters-ratatui-backend), [rasciichart](https://crates.io/api/v1/crates/rasciichart)

## 후보별 분석

### 1. Ratatui 내장 `Sparkline` — 최우선 추천

공식 API는 `Sparkline`을 “한 행 이상에 걸쳐” 데이터 막대를 렌더링하는 위젯으로
설명한다. 데이터는 `u64`, `Option<u64>`, `SparklineBar`를 받을 수 있고 `None`은
0과 구별되는 결측값이다. 최대값, 결측 기호/스타일, 막대 문자 세트도 지정할 수
있다. 기본은 9단계 막대 세트다.

이 특성은 현재 모델과 거의 일대일로 대응한다.

- `Vec<Option<u64>>`로 창 전체의 측정/미측정 열을 그대로 전달할 수 있다.
- `.max(100)`으로 절대 사용률 축을 고정할 수 있다.
- `.absent_value_symbol("·")`로 현재 결측 표현을 유지할 수 있다.
- 4행이면 세로 방향으로 최대 32개 안팎의 막대 단계가 생겨 1행 8단계보다 읽기 쉽다.
- `Widget`이므로 기존 `Frame::render_widget`, 색상, `TestBackend` 테스트를 그대로 쓴다.

주의점은 이것이 선 그래프가 아니라 **각 시점에서 바닥부터 채우는 세로 막대/면적
그래프**라는 점이다. 누적 사용률처럼 대체로 증가하는 값에는 자연스럽지만 작은 증감의
기울기를 읽는 데는 `Chart`의 선이 더 낫다. 또한 항목당 3~5행이 늘어나므로 작은
터미널에서는 차트 높이를 2행으로 낮추거나 숨기는 적응형 레이아웃이 필요하다.

- [Sparkline 0.30.2 API](https://docs.rs/ratatui/0.30.2/ratatui/widgets/struct.Sparkline.html)
- [Ratatui 내장 위젯 목록](https://docs.rs/ratatui/0.30.2/ratatui/widgets/index.html)
- [Ratatui 공식 위젯 쇼케이스](https://ratatui.rs/showcase/widgets/)

통합 난이도: **낮음**. 새 의존성 없이 현재 `Option<String>` 대신 수치 셀과 차트
높이를 렌더러에 넘기면 된다.

### 2. Ratatui 내장 `Chart` + Braille — 선 그래프가 필요할 때

`Chart`는 여러 `Dataset`을 선 또는 산점도로 그리며 축의 bounds와 labels를 설정할
수 있다. `Dataset`의 `GraphType::Line`과 `Marker::Braille`을 쓰면 터미널 한 셀보다
세밀한 선을 얻는다. Ratatui Canvas의 Braille 좌표계는 한 문자 셀을 2×4 점으로
취급한다.

현재 데이터에는 X를 창 시작 이후 분, Y를 0~100으로 주고, bounds를 각각
`[0, window_minutes]`, `[0, 100]`으로 고정하면 된다. 앱 실행 전 구간에는 점 자체가
없으므로 왼쪽이 비어 있고, 첫 측정 이후의 점만 이어진다. 시간 축 레이블이나 0/50/100
기준선을 추가하려는 경우 `Sparkline`보다 확장성이 좋다.

단점은 최소 3~4행에서는 축 레이블이 실제 그래프 공간을 많이 먹고, 축을 숨기면
`Sparkline`보다 설정 코드가 복잡하다는 것이다. 서로 멀리 떨어진 두 관측점을 선으로
잇는 동작이 “조회가 없던 기간”을 실제 관측처럼 보이게 할 수 있으므로, 장시간 조회
실패는 dataset을 구간별로 나누어 선을 끊어야 한다.

- [Chart 0.30.2 API](https://docs.rs/ratatui/0.30.2/ratatui/widgets/struct.Chart.html)
- [Dataset 0.30.2 API](https://docs.rs/ratatui/0.30.2/ratatui/widgets/struct.Dataset.html)
- [Marker 해상도](https://docs.rs/ratatui/0.30.2/ratatui/symbols/enum.Marker.html)

통합 난이도: **중간**. 의존성 문제는 없지만 `History`가 시간 좌표를 포함한
`Vec<(f64, f64)>`와 dataset 수명을 렌더 단계까지 제공해야 한다.

### 3. `tui-bar-graph` — 직접 호환되는 외부 후보

Ratatui 조직의 `tui-widgets` 모노레포에 있는 네이티브 `Widget`이다. `Solid`,
`Quadrant`, `Braille`, `Octant` 막대와 값별/수직 그라데이션을 지원한다. Braille과
Octant는 행당 4단계, Quadrant는 행당 2단계 해상도를 쓰며, 공식 문서는 Braille이
Octant보다 폰트 지원 범위가 넓다고 명시한다.

0.3.5 manifest는 `ratatui-core = 0.1`을 사용하고 모노레포 예제는 `ratatui = 0.30`을
사용하므로 프로젝트의 0.30.2와 맞는다. 그러나 API가 `Vec<f64>`를 받고 결측값
전용 표현이 없으며, Braille 모드는 데이터 두 개를 한 문자 열로 묶는다. 창 전체의
빈 시간 구간을 보존하려면 데이터를 미리 조밀한 배열로 만들고 결측 렌더링 정책을
별도로 정해야 한다. 그라데이션이 중요하지 않다면 내장 `Sparkline` 대비 실질적 이득은
작고 `colorgrad`, `strum`, `tui-bar-graph` 의존성이 늘어난다.

- [tui-bar-graph 0.3.5 API](https://docs.rs/tui-bar-graph/0.3.5/tui_bar_graph/)
- [0.3.5 소스와 문자 패턴](https://github.com/ratatui/tui-widgets/blob/tui-bar-graph-v0.3.5/tui-bar-graph/src/lib.rs)
- [모노레포 manifest](https://github.com/ratatui/tui-widgets/blob/main/Cargo.toml)

통합 난이도: **낮음~중간**. 위젯 타입은 바로 렌더할 수 있지만 결측 시간 구간의
의미를 유지하는 전처리가 필요하다.

### 4. 문자열/CLI 차트 계열

#### `textplots`

Braille canvas 기반 선 그래프이고 백만 회가 넘는 누적 다운로드로 독립 CLI 차트 중
채택도가 가장 높다. 다만 공식 API의 `display()`는 `println!`으로 직접 출력하며,
`Chart::new`는 폭 32점 이상, 높이 3점 이상을 요구한다. `Display` 결과를 문자열로
바꿔 `Paragraph`에 넣을 수는 있지만 축이 포함된 문자열을 Ratatui 영역 크기에 맞추고
ANSI 색을 Ratatui `Style`로 변환하는 별도 어댑터가 필요하다.

- [textplots API](https://docs.rs/textplots/0.8.7/textplots/)
- [공식 저장소](https://github.com/loony-bean/textplots-rs)

통합 난이도: **높음**. 이 프로젝트에서는 내장 `Chart`가 같은 Braille 목적을 더
자연스럽게 달성한다.

#### `asciigraph-rs`

Unicode box-drawing 선, 다중 시리즈, NaN gap, X축 tick/label, ANSI 색상, realtime
CLI를 지원하며 `plot` 결과를 문자열로 반환한다. 기능상 문자열 계열 중 가장 풍부하고
최근에도 유지보수되고 있다. 하지만 2026년 5월에 나온 초기 0.1.x 프로젝트이며 조회
시점 누적 다운로드가 280회뿐이다. ANSI 스타일 문자열은 Ratatui `Paragraph`가
해석하지 않으므로 색을 쓰려면 재파싱해야 한다.

- [공식 저장소와 기능 목록](https://github.com/neneodonkor/asciigraph-rs)
- [asciigraph-rs 0.1.5 API](https://docs.rs/asciigraph-rs/0.1.5/asciigraph/)

통합 난이도: **중간~높음**. 무색 문자열은 넣기 쉽지만 Ratatui 고유 스타일과 테스트
버퍼를 활용하지 못한다.

#### `rasciigraph`

외부 의존성 없이 box-drawing 선 그래프 문자열을 반환하고 높이/폭 및 다중 시리즈를
지원한다. 단순하고 채택 이력도 있지만 Ratatui 위젯이 아니며 기본 출력에 Y축 레이블이
포함된다. 창 전체의 빈 구간과 0~100 고정 범위를 정확히 유지하려면 추가 전처리 또는
크레이트 수정이 필요하다.

- [rasciigraph 0.3.0 API](https://docs.rs/rasciigraph/0.3.0/rasciigraph/)
- [공식 저장소](https://github.com/orhanbalci/rasciigraph)

통합 난이도: **중간**. 문자열 자체는 `Paragraph`에 넣을 수 있으나 레이아웃과 스타일
통합은 수동이다.

#### `rasciichart`

Unicode/ASCII 선, 사용자 지정 크기와 범위를 지원하는 문자열 차트다. 그러나 공식
저장소가 1 star/0 forks이고 Ratatui 통합이 없어, 더 성숙한 `rasciigraph`나 기능이
풍부한 `asciigraph-rs` 대신 선택할 이유가 약하다.

- [공식 저장소](https://github.com/cumulus13/rasciichart)
- [rasciichart API](https://docs.rs/rasciichart/0.2.17/rasciichart/)

통합 난이도: **중간**, 채택 위험: **높음**.

### 5. `plotters-ratatui-backend` — 현 버전에서는 제외

Plotters 차트를 Ratatui `Widget`으로 그리는 백엔드라 축, 선, 다양한 series가 필요한
복잡한 대시보드에는 매력적이다. 그러나 0.3.0 manifest가 `ratatui = "0.29.0"`을
직접 의존한다. Cargo의 caret 규칙상 이는 0.30.2로 올라가지 않으며, 서로 다른
Ratatui 버전의 `Rect`, `Buffer`, `Widget` 타입은 그대로 호환되지 않는다. 현재
프로젝트에서는 Ratatui 0.29를 함께 넣거나 upstream 수정/fork가 필요하고, 단일 사용률
이력을 보여주는 용도에는 Plotters 자체도 과하다.

- [0.3.0 manifest](https://github.com/SOF3/plotters-ratatui-backend/blob/0.3.0/Cargo.toml)
- [공식 저장소](https://github.com/SOF3/plotters-ratatui-backend)

통합 난이도: **현재는 매우 높음/직접 호환 불가**.

## 프로젝트에 적용할 때의 권장 형태

첫 구현은 내장 `Sparkline` 4행을 기준으로 하되 터미널 높이에 따라 적응시키는 것이
안전하다.

- 충분한 높이: 차트 4행
- 제한된 높이: 차트 2행
- 항목조차 다 들어가지 않는 높이: 차트 생략
- 세로축: 항상 `.max(100)`
- 가로축: 기존과 같이 창 전체 폭으로 bucketize
- 결측: `Option<u64>::None`과 `absent_value_symbol("·")`
- 색상: 현재 `Color::Indexed(109)`를 유지해 게이지보다 시각적 우선순위를 낮춤

이 선택은 현재의 의미 체계와 테스트 구조를 유지하면서 가독성만 개선한다. 실제 선의
모양, 시간 tick 또는 기준선이 요구사항으로 확인되면 같은 Ratatui 안에서 `Chart`로
전환하면 된다. `tui-bar-graph` 도입은 그라데이션이나 고밀도 막대 스타일이 명시적인
제품 요구사항이 된 뒤 판단하는 편이 낫다.
