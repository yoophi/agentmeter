# 웹 대시보드

## 실행

```bash
agentmeter web
agentmeter web --live
agentmeter web --interval 120
agentmeter web --port 8080
agentmeter web --host 0.0.0.0 --port 8080
```

`web` 자체가 상주 서버를 뜻하므로 `--watch --web-server`를 함께 쓰지 않습니다. 서버는
기본값은 `127.0.0.1:0`이며 운영체제가 고른 ephemeral port를 씁니다. 고정 포트는
`--port 8080`, 바인딩 IP는 `--host 0.0.0.0`처럼 지정합니다.

`0.0.0.0`은 모든 IPv4 인터페이스에서 요청을 받습니다. 이 서버에는 인증과 TLS가 없으므로
공용 또는 신뢰할 수 없는 네트워크에 직접 노출하지 마세요.

## 두 화면, 한 세션

터미널이 화면으로 쓸 수 있으면 `web`은 서버를 배경으로 돌리고 `--watch`와 같은 TUI를
띄웁니다. 접속 주소는 헤더 둘째 줄에 남아 있어서 실행 로그를 되짚지 않아도 됩니다.

```text
 agentmeter  Asia/Seoul
 web 서버 실행 중  http://127.0.0.1:54321
```

`q`로 나가면 서버도 graceful shutdown 후 함께 내려갑니다. TUI가 alternate screen을
점유하는 동안 서버는 stdout·stderr에 쓰지 않습니다 — 화면이 깨지기 때문입니다.

출력이 터미널이 아니면(파이프, `nohup`, 서비스 실행) TUI 없이 주소만 출력하고 서버만
남습니다.

```text
agentmeter web: http://127.0.0.1:54321
종료하려면 Ctrl-C를 누르세요.
```

설정은 다른 모드와 같은 `~/.config/agentmeter/config.toml`을 읽습니다. `agents`가 하나면
한 pane, 둘이면 같은 순서로 두 pane을 구성합니다.

## 갱신 흐름

조회 상태는 애플리케이션 계층의 `LiveSession`이 소유합니다. 브라우저와 TUI는 그 상태를
읽는 두 개의 화면일 뿐이므로, **화면이 둘이어도 provider 조회는 한 번만 나갑니다.**

1. 전용 스레드 하나가 `LiveSession::run_refresh_loop`를 돌립니다 — 즉시 한 번 조회한 뒤
   주기마다 반복합니다.
2. 결과는 `WatchState`에 반영되어 이전 성공 값, 오류, 창별 히스토리를 두 화면이 공유합니다.
3. 브라우저는 `GET /api/dashboard`를 2초마다 읽고, TUI는 매초 같은 상태를 다시 그립니다.
4. 자동 provider 조회는 기본 60초, 최소 30초입니다.

새로고침 요청은 두 경로로 들어옵니다. TUI의 `r`·`R` 키는 화면을 멈출 수 없으므로
`LiveSession::request`로 큐에 넣고 조회 루프가 집어갑니다. `POST /api/refresh`는
요청-응답으로 결과를 돌려줘야 하므로 `refresh_blocking`을 `spawn_blocking`에서
실행합니다. 어느 경로든 refresh gate가 provider 조회를 직렬화하고, 대기 중 요청은
하나로 병합되며 `Fresh`가 우선합니다.

`agentmeter web --live`는 자동 조회마다 `FetchPolicy::Fresh`를 사용합니다. 화면의
`Refresh` 버튼은 캐시 우선, `Live HTTP` 버튼은 한 번만 강제 직접 조회합니다.

서버 시작 시 `WatchState`는 현재 활성 상태인 5시간/7일 이력 파일을 먼저 읽습니다. 따라서
Claude HTTP 요청이 처음부터 실패하더라도 마지막 실측값과 area chart를 표시하고, 해당 값이
로컬 이력이라는 점과 최신 조회 오류를 함께 보여줍니다.

## 화면

- CSS bar chart: 현재 사용률과 심각도
- SVG area chart: 현재 5시간/7일 창의 실제 측정 히스토리. 기록이 없는 구간은 이어 그리지
  않고 비워 둡니다 — 조회가 멈춘 시간을 직선으로 잇는 것은 없는 측정을 있는 것처럼
  보이게 하기 때문입니다. 표본 간격의 중앙값을 실제 조회 주기로 보고 그 몇 배를 넘는
  결측만 공백으로 판정하되, 짧은 결측(조회 지연·재시작)은 끊지 않습니다.
- 오류 banner: HTTP 429 등을 표시하면서 직전 성공 데이터 유지
- responsive pane: 넓은 화면에서는 1~2열, 좁은 화면에서는 한 열
- chrome 숨김: 우상단 눈 아이콘으로 header·footer를 접습니다. 남는 것은 데이터뿐이므로
  표시 영역을 화면 전체의 중앙에 놓습니다 — 모니터를 상시 대시보드로 쓰는 경우입니다.
  내용이 화면보다 높으면 중앙 정렬 대신 위에서 시작해 스크롤로 전부 볼 수 있게 합니다.

차트 라이브러리나 CDN은 사용하지 않습니다. 대시보드 자산은 실행 파일에 포함됩니다.

## 네트워크 범위

기본 설정은 loopback IPv4라 같은 컴퓨터에서만 접근할 수 있습니다. 다른 기기에서는
SSH 터널을 권장하며, 신뢰할 수 있는 LAN에서는 명시적으로 `--host 0.0.0.0`을 사용할 수
있습니다. 원격 공개용 인증·TLS 서버는 아닙니다.
