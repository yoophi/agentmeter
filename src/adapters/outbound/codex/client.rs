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
use crate::application::FetchError;

#[cfg(not(test))]
const TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(test)]
const TIMEOUT: Duration = Duration::from_secs(1);
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
            format!("could not run `{bin} app-server`. check that the Codex CLI is installed")
        })
}

fn talk(child: &mut Child) -> Result<RateLimitsResponse, FetchError> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| FetchError::Other(anyhow!("could not open the app-server stdin")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| FetchError::Other(anyhow!("could not open the app-server stdout")))?;

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

    let messages = handshake();
    let deadline = Instant::now() + TIMEOUT;
    send(&mut stdin, &messages[0])?;
    let mut initialized = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(FetchError::Other(anyhow!(
                "the app-server did not answer within {}s",
                TIMEOUT.as_secs()
            )));
        }
        let line = match rx.recv_timeout(remaining) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(FetchError::Other(anyhow!(
                    "the app-server did not answer within {}s",
                    TIMEOUT.as_secs()
                )));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(FetchError::Other(anyhow!(
                    "the app-server exited before finishing its answer"
                )));
            }
        };

        if !initialized {
            if let Some(result) = response_result(&line, 1) {
                result?;
                for message in &messages[1..] {
                    send(&mut stdin, message)?;
                }
                initialized = true;
            }
        } else if let Some(result) = take_response(&line) {
            return result;
        }
    }
}

fn send(writer: &mut impl Write, message: &str) -> Result<(), FetchError> {
    writer
        .write_all(message.as_bytes())
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .context("could not send the request to the app-server")
        .map_err(FetchError::Other)
}

fn handshake() -> Vec<String> {
    vec![
        serde_json::json!({
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {
                "name": "agentmeter",
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
    response_result(line, REQUEST_ID).map(|result| {
        result.and_then(|value| {
            serde_json::from_value(value)
                .context("could not parse the rateLimits response")
                .map_err(FetchError::Other)
        })
    })
}

fn response_result(line: &str, expected_id: i64) -> Option<Result<Value, FetchError>> {
    let mut v: Value = serde_json::from_str(line).ok()?;
    if v.get("id").and_then(Value::as_i64) != Some(expected_id) {
        return None;
    }
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string();
        let lower = msg.to_lowercase();
        if lower.contains("auth") || lower.contains("login") || lower.contains("unauthorized") {
            return Some(Err(FetchError::Unauthorized(format!(
                "{msg} — sign in with `codex login`"
            ))));
        }
        return Some(Err(FetchError::Other(anyhow!("app-server error: {msg}"))));
    }
    Some(v.get_mut("result").map(Value::take).ok_or_else(|| {
        FetchError::Other(anyhow!(
            "the app-server response has no result (id={expected_id})"
        ))
    }))
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
