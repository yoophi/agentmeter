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
기본값은 `127.0.0.1:0`이며 운영체제가 고른 ephemeral port를 출력합니다. 고정 포트는
`--port 8080`, 바인딩 IP는 `--host 0.0.0.0`처럼 지정합니다.

`0.0.0.0`은 모든 IPv4 인터페이스에서 요청을 받습니다. 이 서버에는 인증과 TLS가 없으므로
공용 또는 신뢰할 수 없는 네트워크에 직접 노출하지 마세요.

```text
agentmeter web: http://127.0.0.1:54321
종료하려면 Ctrl-C를 누르세요.
```

설정은 다른 모드와 같은 `~/.config/agentmeter/config.toml`을 읽습니다. `agents`가 하나면
한 pane, 둘이면 같은 순서로 두 pane을 구성합니다.

## 갱신 흐름

1. Tokio background task가 즉시 한 번 조회합니다.
2. 기존 동기 `UsageApplication::query`는 `spawn_blocking`에서 실행합니다.
3. 결과를 기존 `WatchState`에 반영해 이전 성공 값, 오류, 창별 히스토리를 공유합니다.
4. 브라우저는 `GET /api/dashboard`를 2초마다 읽어 화면을 갱신합니다.
5. 자동 provider 조회는 기본 60초, 최소 30초입니다.

`agentmeter web --live`는 자동 조회마다 `FetchPolicy::Fresh`를 사용합니다. 화면의
`Refresh` 버튼은 캐시 우선, `Live HTTP` 버튼은 한 번만 강제 직접 조회합니다. 동시에
여러 요청이 들어와도 refresh gate가 provider 조회를 직렬화합니다.

서버 시작 시 `WatchState`는 현재 활성 상태인 5시간/7일 이력 파일을 먼저 읽습니다. 따라서
Claude HTTP 요청이 처음부터 실패하더라도 마지막 실측값과 area chart를 표시하고, 해당 값이
로컬 이력이라는 점과 최신 조회 오류를 함께 보여줍니다.

## 화면

- CSS bar chart: 현재 사용률과 심각도
- SVG area chart: 현재 5시간/7일 창의 실제 측정 히스토리
- 오류 banner: HTTP 429 등을 표시하면서 직전 성공 데이터 유지
- responsive pane: 넓은 화면에서는 1~2열, 좁은 화면에서는 한 열

차트 라이브러리나 CDN은 사용하지 않습니다. 대시보드 자산은 실행 파일에 포함됩니다.

## 네트워크 범위

기본 설정은 loopback IPv4라 같은 컴퓨터에서만 접근할 수 있습니다. 다른 기기에서는
SSH 터널을 권장하며, 신뢰할 수 있는 LAN에서는 명시적으로 `--host 0.0.0.0`을 사용할 수
있습니다. 원격 공개용 인증·TLS 서버는 아닙니다.
