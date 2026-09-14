# GLM Coding Plan 연동

## 무엇을 읽는가

Z.ai GLM Coding Plan의 **계정 쿼터**를 읽습니다. 로컬 도구 로그가 아니라 계정 단위
값이므로 여러 기기·클라이언트에서 쓴 양이 모두 합산되어 있습니다.

```bash
agentmeter --agent glm
```

```text
› Current session
  ██░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  5% used
  ██████████████████████████████████████████░░░░░░  0 hour 37 minutes left
  Resets Sep 14 at 5:17pm (Asia/Seoul)

  Current month (MCP tools)
  ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  0% used
  ████████████████████████░░░░░░░░░░░░░░░░░░░░░░░░  14 day 17 hour 1 minutes left
  Resets Sep 29 at 9:40am (Asia/Seoul)
```

## API 키

새 설정을 만들지 않습니다. 다음 순서로 찾고, 먼저 발견한 것을 씁니다.

1. `ZAI_API_KEY` 환경변수
2. `GLM_TRACKER_ZAI_KEY` 환경변수
3. `~/.local/share/opencode/auth.json` 의 `zai-coding-plan.key`

이미 GLM을 쓰고 있다면 대개 3번에 키가 있습니다. 키를 찾지 못하면 다른 provider는
그대로 두고 이 pane에만 안내 문구를 띄웁니다.

## 엔드포인트

```
GET https://api.z.ai/api/monitor/usage/quota/limit
Authorization: <API_KEY>
Accept-Language: en-US,en
```

**공식 문서에 없는 내부 엔드포인트입니다.** Z.ai 공식 플러그인(`glm-plan-usage`)이
쓰는 경로이며, 공개 API 레퍼런스에는 계정 수준 사용량 조회가 존재하지 않습니다.
따라서 스키마 변경·401·429를 모두 오류 상태로 표현하고, 직전 성공 값을 보존합니다.

`Authorization`은 `Bearer` 접두어 없이 키를 그대로 보냅니다. 실측상 접두어를 붙여도
200이 오지만, 공식 플러그인과 같은 형태를 씁니다.

## 응답과 한도 매핑

```jsonc
{"code":200,"data":{"limits":[
  {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":4,
   "nextResetTime":1789373822847},
  {"type":"TIME_LIMIT","unit":5,"number":1,"usage":4000,"currentValue":0,
   "percentage":0,"nextResetTime":1790642457999}],"level":"max"}}
```

| 응답 | limit id | 창 | 화면 |
|---|---|---|---|
| `TOKENS_LIMIT` | `glm:tokens` | `number`×`unit` = 5시간 | `Current session` (강조) |
| `TIME_LIMIT` | `glm:tools` | 1개월 | `Current month (MCP tools)` |

`unit`은 실측으로 확인한 값만 창 길이로 옮깁니다 — **3=시간, 5=월**. 리셋 시각을
역산해 확인했습니다(5시간 창은 리셋까지 0.81시간, 월 창은 14.7일). 모르는 단위가
오면 창 길이만 비우므로 시간 게이지가 빠지고 소진율과 리셋 시각은 그대로 나옵니다.

달은 길이가 일정하지 않아 30일로 근사합니다. 경과 게이지용 근사이고, 창을 가르는
기준은 리셋 시각이라 히스토리가 섞이지 않습니다.

### limit id는 자리가 아니라 종류로 정합니다

응답 순서가 바뀌어도 `glm:tokens` / `glm:tools`는 그대로입니다. 자리(첫째/둘째)를
id에 넣으면 공급자가 순서를 바꿀 때 히스토리가 끊깁니다.

화면에서는 창이 짧은 쪽을 위에 둡니다. 자주 보는 값이 위에 있어야 합니다.

## 캐시

원격 호출이라 1분 캐시를 둡니다. `r`(캐시 우선)은 이 캐시를 쓰고, `R`(직접 조회)은
건너뜁니다. 조회가 실패하면 직전 성공 값을 `refresh_failed` 상태로 계속 보여줍니다.

## 로컬 집계와의 차이

이 provider는 **남은 한도**를 봅니다. "얼마나 태웠는지"(누적 토큰)는 계정 쿼터에서
얻을 수 없고, 반대로 로컬 도구 로그로는 플랜 소진율을 알 수 없습니다. 두 숫자는
의미가 달라 서로 환산되지 않습니다.

머신·도구별 토큰 분해가 필요하면 로컬 로그를 읽는 별도 도구를 쓰세요.
agentmeter는 provider의 로컬 대화 로그를 집계하지 않습니다.
