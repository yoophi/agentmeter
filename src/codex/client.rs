//! `codex app-server` 를 자식 프로세스로 띄워 한도를 조회한다.
//!
//! HTTP 엔드포인트를 직접 두드리지 않고 Codex 가 공식으로 제공하는
//! app-server 프로토콜(JSONL)을 쓴다. 토큰 갱신·재시도는 Codex 가 처리하므로
//! 이 쪽에서 자격증명을 만질 일이 없다.
//!
//! 프로토콜은 `codex app-server generate-json-schema` 가 내보내는
//! `ClientRequest` 정의를 따른다. 줄 단위 JSON 이고 `jsonrpc` 필드는 없다.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use serde_json::Value;

use super::model::RateLimitsResponse;
use crate::FetchError;

const TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_ID: i64 = 2;

/// `codex` 실행 파일. `CODEX_BIN` 으로 바꿀 수 있다.
fn codex_bin() -> String {
    std::env::var("CODEX_BIN").unwrap_or_else(|_| "codex".to_string())
}

pub fn fetch() -> Result<RateLimitsResponse, FetchError> {
    let mut child = spawn().map_err(FetchError::Other)?;
    let result = talk(&mut child);
    // 성공하든 실패하든 자식 프로세스를 남기지 않는다
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn spawn() -> anyhow::Result<Child> {
    let bin = codex_bin();
    Command::new(&bin)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| {
            format!(
                "`{bin} app-server` 를 실행할 수 없습니다. Codex CLI 가 설치되어 있는지 확인하세요"
            )
        })
}

fn talk(child: &mut Child) -> Result<RateLimitsResponse, FetchError> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| FetchError::Other(anyhow!("app-server stdin 을 열 수 없습니다")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| FetchError::Other(anyhow!("app-server stdout 을 열 수 없습니다")))?;

    // 읽기는 별도 스레드에서. 응답이 오지 않을 때 영원히 매달리지 않도록
    // 메인은 채널을 마감 시각까지만 기다린다.
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });

    for line in handshake() {
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .context("app-server 로 요청을 보내지 못했습니다")
            .map_err(FetchError::Other)?;
    }
    stdin.flush().ok();

    let deadline = Instant::now() + TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(FetchError::Other(anyhow!(
                "app-server 가 {}초 안에 응답하지 않았습니다",
                TIMEOUT.as_secs()
            )));
        }
        let line = rx.recv_timeout(remaining).map_err(|_| {
            FetchError::Other(anyhow!("app-server 가 응답을 마치기 전에 종료되었습니다"))
        })?;

        // 알림(notification)이 섞여 오므로 우리 요청 id 만 골라낸다
        match take_response(&line) {
            Some(res) => return res,
            None => continue,
        }
    }
}

fn handshake() -> Vec<String> {
    vec![
        serde_json::json!({
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {
                "name": "codexmeter",
                "version": env!("CARGO_PKG_VERSION"),
            }},
        })
        .to_string(),
        serde_json::json!({"method": "initialized"}).to_string(),
        serde_json::json!({"id": REQUEST_ID, "method": "account/rateLimits/read"}).to_string(),
    ]
}

/// 한 줄을 보고 우리 응답이면 결과를, 아니면 `None` 을 돌려준다.
fn take_response(line: &str) -> Option<Result<RateLimitsResponse, FetchError>> {
    let mut v: Value = serde_json::from_str(line).ok()?;
    if v.get("id").and_then(Value::as_i64) != Some(REQUEST_ID) {
        return None;
    }
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("알 수 없는 오류")
            .to_string();
        // 로그인 문제는 재인증 안내로 돌려준다
        let lower = msg.to_lowercase();
        if lower.contains("auth") || lower.contains("login") || lower.contains("unauthorized") {
            return Some(Err(FetchError::Unauthorized(format!(
                "{msg} — `codex login` 으로 로그인하세요"
            ))));
        }
        return Some(Err(FetchError::Other(anyhow!("app-server 오류: {msg}"))));
    }
    let result = v.get_mut("result")?.take();
    Some(
        serde_json::from_value(result)
            .context("rateLimits 응답 파싱 실패")
            .map_err(FetchError::Other),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_matches_the_protocol() {
        let lines = handshake();
        assert_eq!(lines.len(), 3);
        let init: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(init["method"], "initialize");
        // clientInfo 는 필수 필드다
        assert!(init["params"]["clientInfo"]["name"].is_string());
        assert!(init["params"]["clientInfo"]["version"].is_string());

        let ready: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(ready["method"], "initialized");
        assert!(ready.get("id").is_none(), "알림에는 id 가 없어야 함");

        let req: Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(req["method"], "account/rateLimits/read");
        assert_eq!(req["id"], REQUEST_ID);
    }

    /// 초기화 응답과 알림이 섞여 와도 우리 요청만 집어야 한다.
    #[test]
    fn ignores_other_lines() {
        assert!(take_response(r#"{"id":1,"result":{"codexHome":"/x"}}"#).is_none());
        assert!(
            take_response(r#"{"method":"remoteControl/status/changed","params":{}}"#).is_none()
        );
        assert!(take_response("not json at all").is_none());
    }

    #[test]
    fn picks_our_response() {
        let line = r#"{"id":2,"result":{"rateLimits":{"limitId":"codex",
            "primary":{"usedPercent":50,"windowDurationMins":10080,"resetsAt":1787196678}}}}"#;
        let got = take_response(line).expect("우리 응답이어야 함").unwrap();
        assert_eq!(got.rate_limits.primary.unwrap().used_percent, 50.0);
    }

    #[test]
    fn auth_errors_become_reauth_hints() {
        let line = r#"{"id":2,"error":{"code":-32000,"message":"not logged in: auth required"}}"#;
        match take_response(line).unwrap() {
            Err(FetchError::Unauthorized(m)) => assert!(m.contains("codex login")),
            other => panic!("재인증 안내여야 함: {other:?}"),
        }
    }

    #[test]
    fn other_errors_are_reported_as_is() {
        let line = r#"{"id":2,"error":{"code":-32603,"message":"internal boom"}}"#;
        match take_response(line).unwrap() {
            Err(FetchError::Other(e)) => assert!(format!("{e:#}").contains("internal boom")),
            other => panic!("일반 오류여야 함: {other:?}"),
        }
    }
}
