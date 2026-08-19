# agentmeter

코딩 에이전트의 사용 한도를 같은 형식으로 보여주는 CLI 모음입니다.

- **`agentmeter`** — 설정한 에이전트들을 한 화면에 나란히
- **`ccmeter`** — Claude Code 만
- **`codexmeter`** — Codex 만
각 도구의 `/usage`·`/status` 가 보여주는 것과 같은 값 — 한도 소진율과 리셋 시각 — 을
터미널에서 바로 확인하거나 상주시켜 둘 수 있습니다.

```
$ ccmeter
› Current session
  ██████████████████████████████████████░░░░░░░░░░  79% used
  ███████████████████████████████████████████████░  0 hour 4 minutes left
  Resets Aug 18 at 9:30pm (Asia/Seoul)

  Current week (all models)
  ███████████████████████████░░░░░░░░░░░░░░░░░░░░░  57% used
  ███████████████████████████████████████████████░  0 day 3 hour 34 minutes left
  Resets Aug 19 at 1:00am (Asia/Seoul)

  기준 21:23 (1분 전, 로컬 캐시)

$ codexmeter
  Current week (all models)
  ████████████████████████░░░░░░░░░░░░░░░░░░░░░░░░  50% used
  █████████████████████████████████████░░░░░░░░░░░  1 day 15 hour 5 minutes left
  Resets Aug 20 at 12:31pm (Asia/Seoul)

  기준 21:25 (방금, 직접 조회)
```

두 도구는 같은 옵션, 같은 게이지, 같은 문구를 씁니다.
리셋 시각은 오늘이든 아니든 항상 날짜를 붙입니다 — 시각만 있으면 화면만 보고
오늘인지 내일인지 알 수 없습니다.
Codex 의 `/status` 는 남은 비율(`71% left`)로 표시하지만, 여기서는 ccmeter 와 맞춰
**소진율**(`29% used`)로 통일했습니다. `›` 는 지금 적용 중인 한도 표시입니다.

게이지는 두 줄입니다. 위는 한도를 얼마나 썼는지, 아래는 그 창(세션 5시간 / 주간 7일)의
시간이 얼마나 흘렀는지입니다. 둘을 견주면 페이스가 읽힙니다 — 위 예시는 시간이 95% 지났는데
79% 만 썼으니 여유가 있습니다. 반대로 시간 게이지가 더 짧으면 이번 창은 일찍 소진됩니다.

마지막 줄은 그 숫자가 **언제 기준인지** 밝힙니다. ccmeter 는 로컬 캐시를 읽을 수 있어서
방금 조회한 값인지 몇 분 전 값인지 구분되어야 합니다.

## 사용법

### agentmeter — 여러 에이전트를 한 화면에

```bash
agentmeter                              # 설정한 에이전트를 모두 조회
agentmeter -w                           # 상주 모드 (좌우 분할)
agentmeter --json                        # 에이전트별 키를 가진 JSON

agentmeter config list                   # 설정 보기
agentmeter config get agents             # 값 하나 보기
agentmeter config set agents=claude,codex   # 표시할 에이전트 지정
```

구획은 **좌우로** 놓입니다 (`A | B`). 터미널이 좁으면(100칸 미만) 1회 출력에서는
세로로 쌓습니다 — 폭 40짜리 게이지 두 개보다 폭 80짜리 하나가 읽기 쉽습니다.

에이전트 하나가 실패해도 나머지는 그대로 보여주고, 조회는 **동시에** 합니다.

설정은 `~/.config/agentmeter/config.toml` 에 저장됩니다
(`XDG_CONFIG_HOME` 을 존중합니다).

```toml
agents = ["claude", "codex"]
```

순서가 화면 순서입니다. 파일이 없으면 등록된 에이전트를 모두 보여줍니다.
모르는 이름을 넣으면 **저장하기 전에** 거절합니다.

### ccmeter / codexmeter — 한 에이전트만

```bash
ccmeter                    # 한 번 출력하고 종료
ccmeter -w                 # 상주 모드 (전체 화면, 60초마다 갱신)
ccmeter -n 120             # 120초 주기로 상주
ccmeter --json             # statusline·스크립트용
ccmeter --live             # 캐시를 건너뛰고 직접 조회 (ccmeter 전용)
watch -n 60 ccmeter        # watch 와 함께 써도 됩니다
```

`codexmeter` 도 옵션이 완전히 같습니다.
상주 모드에서 `r` 은 즉시 새로고침, `q` 는 종료입니다.

상주 모드는 조회할 때마다 사용률을 분 단위로 기록해 한 줄 차트로 보여줍니다.

```
 › Current session  +3%p
   ██████████████████████████░░░░░░░░  62% used
   ████████████████████████████████░░  0 hour 24 minutes left
   ·····························▅▅▆··
   Resets Aug 18 at 9:30pm (Asia/Seoul)
```

가로축은 창 전체(세션 5시간 / 주간 7일)라 바로 위 시간 게이지와 축이 같고,
앱을 켜기 전 구간은 `·` 로 비어 있습니다. 제목 옆 `+3%p` 는 켠 뒤로 늘어난 양입니다.
기록은 메모리에만 남으므로 1회 실행에는 차트가 나오지 않습니다.

출력이 터미널이 아니면(파이프, `watch` 아래) 자동으로 1회 출력으로 내려갑니다.
전체 화면 TUI 는 alternate screen 을 쓰기 때문에 `watch` 안에서는 동작할 수 없습니다.

## 동작 방식

### ccmeter

**로컬 캐시를 먼저 읽습니다.** Claude Code 는 `/api/oauth/usage` 응답을 통째로
`~/.claude/token-scope-oauth-usage.json` 에 저장해 두는데, 그 파일을 읽으면
네트워크 호출이 없어 즉시 응답하고 `HTTP 429` 도 없습니다.

캐시가 15분보다 오래됐을 때만 `GET /api/oauth/usage` 를 직접 호출합니다.
조회에 실패하면 5분간 다시 시도하지 않고, 그동안은 캐시 값에 `갱신 실패` 를 붙여
보여줍니다. `--live` 는 캐시를 건너뛰고 항상 직접 조회합니다.

직접 조회할 때 자격증명은 macOS Keychain(`Claude Code-credentials`), 없으면
`~/.claude/.credentials.json` 에서 **읽기만** 합니다.
토큰 갱신은 일부러 하지 않습니다 — 리프레시 토큰은 회전(rotation)하므로 이 도구가 갱신해
저장하면 Claude Code 본체의 세션을 무효화할 수 있습니다. 대신 조회할 때마다 자격증명을 다시
읽어서, Claude Code 가 갱신해 둔 새 토큰을 자연스럽게 따라갑니다.
만료되면 `claude` 를 한 번 실행하면 됩니다.

자세한 내용은 [docs/ccmeter.md](docs/ccmeter.md) 를 보세요.

### codexmeter

`codex app-server` 를 자식 프로세스로 띄우고 `account/rateLimits/read` 를 호출합니다.
HTTP 엔드포인트를 직접 두드리지 않으므로 토큰 관리·갱신은 전부 Codex 가 처리합니다.
`CODEX_BIN` 으로 실행 파일 경로를 바꿀 수 있습니다.

프로토콜과 응답 타입은 Codex 가 스스로 내보내는 스키마를 따릅니다:

```bash
codex app-server generate-json-schema --out ./schema
# GetAccountRateLimitsResponse, RateLimitSnapshot, RateLimitWindow
```

`rateLimitsByLimitId`(다중 버킷)가 있으면 그쪽을 쓰고, 없을 때만 하위호환용
`rateLimits` 단일 뷰로 내려갑니다. 둘을 함께 쓰면 기본 한도가 두 번 나옵니다.
주간 창만 보여주며, 호출이 저렴해 캐시 계층 없이 매번 직접 조회합니다.

자세한 내용은 [docs/codexmeter.md](docs/codexmeter.md) 를 보세요.

### 로컬 로그를 쓰지 않는 이유

두 도구 모두 로컬 로그(`~/.claude/projects/**/*.jsonl` 등)를 집계하지 **않습니다.**
로컬 로그에는 그 기기에서 쓴 기록만 남지만 한도는 계정 단위로 합산되므로,
다른 컴퓨터나 웹에서 쓴 양이 빠져 항상 실제보다 낮게 나옵니다.
서버가 판정한 값을 그대로 받아오는 편이 정확합니다.

### 갱신 주기

상주 모드의 갱신 주기는 최소 30초입니다(기본 60초). 데이터 자체가 그보다 자주 변하지 않습니다.

`/api/oauth/usage` 자체에도 한도가 있어 자주 부르면 `HTTP 429` 가 돌아옵니다.
ccmeter 가 캐시를 먼저 읽는 것도 이 때문입니다 — 기본 경로에서는 네트워크를 쓰지 않습니다.
서버가 `Retry-After` 를 주면 그 값을 그대로 안내하지만, 실제로는 `retry-after: 0` 을
주면서 계속 막는 경우가 있어 0 은 "값 없음"으로 취급합니다.
상주 모드는 다음 주기에 자연히 회복되므로 따로 할 일이 없습니다.

## 주의

- ccmeter 가 쓰는 `/api/oauth/usage` 는 **공개 문서가 없는 내부 엔드포인트**입니다.
  Anthropic 이 언제든 응답 형태를 바꾸거나 없앨 수 있습니다.
- codexmeter 가 쓰는 app-server 는 Codex 가 `[experimental]` 로 표시한 인터페이스입니다.

그래서 양쪽 모두 개별 필드를 하드코딩하지 않고, 서버가 정규화해 둔 목록
(`limits` / `rateLimitsByLimitId`)을 순회합니다. 항목이 늘거나 처음 보는 종류가 와도
그대로 표시되며 죽지 않습니다.

## 문서

- [docs/architecture.md](docs/architecture.md) — 공통 구조, `Meter` 표현, 실행 모드
- [docs/ccmeter.md](docs/ccmeter.md) — 캐시 우선 조회, 자격증명, 429 대응
- [docs/codexmeter.md](docs/codexmeter.md) — app-server 프로토콜, 응답 처리

## 구조

```
src/
  meter.rs        공통 화면 표현 (제목·게이지 두 줄·각주) + 문구·창 진행률 계산
  app.rs          모드 분기와 실행 로직
  cli.rs          공통 옵션
  history.rs      실행 후 사용률 변화와 텍스트 차트 (상주 모드)
  config.rs       설정 파일 (표시할 에이전트)
  registry.rs     이름 → 에이전트 표
  multi.rs        여러 에이전트 동시 조회
  render/         plain(stdout) · tui(ratatui) 렌더러 · JSON 출력
  claude/         api(HTTP) · auth(Keychain) · source(캐시 우선) · model
  codex/          client(app-server) · source · model
  bin/            ccmeter.rs · codexmeter.rs
```

각 도구는 자기 응답을 `Meter` 로 옮기기만 하고, 게이지·TUI·CLI 는 전부 공유합니다.
새 에이전트를 추가하려면 `src/<이름>/` 에 클라이언트와 `to_meters()`,
그리고 `Snapshot` 을 돌려주는 `source::fetch(tz)` 를 만든 뒤
`registry.rs` 에 한 줄 등록하면 됩니다. 그러면 설정 파일과 `agentmeter` 가
바로 알아봅니다. 전용 바이너리를 따로 두고 싶으면 `src/bin/` 에 진입점을 추가합니다.
자세한 것은 [docs/architecture.md](docs/architecture.md) 를 보세요.

## 설치

```bash
cargo install --path .   # ccmeter, codexmeter 두 개가 함께 설치됩니다
```

## 빌드

```bash
cargo build --release
cargo test
```
