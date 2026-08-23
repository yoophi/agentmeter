# Kiro provider CLI 연동

agentmeter는 로그인된 공식 Kiro CLI가 보여주는 월간 구독 credit을 수집합니다.
비공개 management API나 로컬 인증 credential은 직접 사용하지 않습니다.

## 조회 명령

```bash
kiro-cli chat --no-interactive "/usage"
```

기본 실행 파일은 `kiro-cli`이며 테스트나 별도 설치 경로가 필요하면 `KIRO_BIN`으로
바꿀 수 있습니다. 프로세스가 20초 안에 끝나지 않으면 종료해 상주 monitor가 멈추지
않도록 합니다.

ANSI 스타일을 제거한 뒤 다음 정보를 파싱합니다.

- 구독 plan
- 사용 credit과 월간 한도
- reset 날짜
- overage 활성화 여부(출력에 있을 때만)

조직 계정은 `Overages` 행 대신 관리자 안내를 출력할 수 있으므로 overage 상태는
선택값입니다.

## 화면과 JSON

사용 credit은 공통 `UsageLimit`의 퍼센트로 정규화되어 기존 TUI·웹 history chart에
기록됩니다. provider가 공개한 원래 숫자는 `quota`로 함께 보존하여 CLI label과 JSON에서
사용량, 한도, 잔여량을 잃지 않습니다.

```json
{
  "title": "Current month (KIRO POWER)",
  "percent": 2.7,
  "label": "3% used",
  "quota_summary": "271.77 / 10,000 credits · 9,728.23 left · daily budget 1,216.03/day",
  "quota": {
    "used": 271.77,
    "limit": 10000.0,
    "remaining": 9728.23,
    "unit": "credits",
    "safe_daily_budget": 1216.03
  }
}
```

Safe daily budget은 현재 잔여 credit을 reset까지 남은 온전한 일수로 나누며, reset이
하루보다 적게 남으면 하루를 최소 분모로 사용합니다.

## 갱신 정책

Kiro CLI 조회는 같은 프로세스 안에서 5분 동안 메모리에 캐시합니다. 따라서 전체 화면이
60초마다 갱신돼도 실제 Kiro subprocess는 5분마다 한 번만 실행됩니다.

- 일반 조회와 `r`: 5분 캐시 우선
- `--live`와 `R`: 즉시 CLI 실행
- 캐시 갱신 실패: 직전 snapshot을 `갱신 실패` 상태로 유지

프로세스를 다시 시작하면 첫 조회는 항상 CLI를 실행합니다.

## 범위

현재 구현은 account-level 월간 credit을 Source of Truth로 표시합니다.
`~/.kiro/sessions/`의 turn별 `usage_summary`를 읽는 project/session analytics와 ACP
metering metadata 수집은 포함하지 않습니다. 로컬 기록은 다른 컴퓨터나 웹에서 사용한
credit을 포함하지 않으므로 account 잔여량을 대체해서도 안 됩니다.
