//! `pdfcraft-cli run`, and `mcp` only with `--remote-signing`, give the automation tools the
//! HTTPS transport, so remote signing reaches the service address a call names. The service here
//! is a plain HTTP server on 127.0.0.1 (all `check_url` allows without https) whose `info` offers
//! only OAuth sign-in: one real exchange, and the answer read back, without the password ever
//! being sent.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

const PASSWORD: &str = "correct horse battery";

/// What the CLI says when the exchange went through and the service turned out OAuth-only.
const OAUTH_ONLY: &str = "this signing service can't be used: it needs OAuth sign-in in a browser, which PdfCraft doesn't support yet";

/// The service's base address and the head (request line and headers) of each request it read.
fn oauth_only_service() -> (String, Arc<Mutex<Vec<Vec<String>>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/csc/v2", listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let mut reader = BufReader::new(&stream);
            let mut head = Vec::new();
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && !line.trim_end().is_empty() {
                head.push(line.trim_end().to_string());
                line.clear();
            }
            let length = head
                .iter()
                .find_map(|h| h.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                .unwrap_or(0);
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            log.lock().unwrap().push(head);
            let answer = r#"{"specs":"2.0.0.0","name":"OAuth only","authType":["oauth2code"]}"#;
            let reply =
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
            let _ = (&stream).write_all(reply.as_bytes());
        }
    });
    (url, seen)
}

/// The service saw exactly `n` requests, each the `info` that comes before any sign-in, from
/// PdfCraft and without credentials.
fn assert_only_info(seen: &Mutex<Vec<Vec<String>>>, n: usize) {
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), n, "{seen:?}");
    for head in seen.iter() {
        assert_eq!(head.first().map(String::as_str), Some("POST /csc/v2/info HTTP/1.1"), "{head:?}");
        assert!(head.iter().any(|h| h.to_ascii_lowercase().starts_with("user-agent: pdfcraft/")), "{head:?}");
        assert!(!head.iter().any(|h| h.to_ascii_lowercase().starts_with("authorization:")), "{head:?}");
    }
}

#[test]
fn run_reaches_the_signing_service() {
    let (url, seen) = oauth_only_service();
    let out = Command::new(BIN)
        .args(["run", "sign_remote_ids", "protocol=csc", &format!("url={url}"), "user=mario.rossi", &format!("password={PASSWORD}")])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains(OAUTH_ONLY), "{stderr}");
    assert!(!stderr.contains(PASSWORD), "{stderr}");
    assert_only_info(&seen, 1);
}

/// A digits-only value for a string argument stays the text typed: a numeric user name and
/// password reach the service instead of being refused as numbers.
#[test]
fn run_keeps_digits_typed_for_a_string_argument() {
    let (url, seen) = oauth_only_service();
    let out =
        Command::new(BIN).args(["run", "sign_remote_ids", "protocol=csc", &format!("url={url}"), "user=1234", "password=123456"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains(OAUTH_ONLY) && !stderr.contains("123456"), "{stderr}");
    assert_only_info(&seen, 1);
}

/// `mcp` with `args`: the reply to a `sign_remote_ids` call against `url`, and what was printed.
#[cfg(feature = "mcp")]
fn mcp_sign_remote_ids(args: &[&str], url: &str) -> (serde_json::Value, String, String) {
    use std::process::Stdio;
    let mut child = Command::new(BIN).arg("mcp").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let call = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "sign_remote_ids",
        "arguments": { "protocol": "csc", "url": url, "user": "mario.rossi", "password": PASSWORD } } });
    let input = format!("{{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{{}}}}\n{call}\n");
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let reply: serde_json::Value = serde_json::from_str(stdout.lines().nth(1).unwrap()).unwrap();
    (reply, stdout, String::from_utf8(out.stderr).unwrap())
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_reaches_the_signing_service_with_remote_signing() {
    let (url, seen) = oauth_only_service();
    let (reply, stdout, stderr) = mcp_sign_remote_ids(&["--remote-signing"], &url);
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    assert_eq!(reply["result"]["content"][0]["text"], OAUTH_ONLY, "{reply}");
    assert!(!stdout.contains(PASSWORD) && !stderr.contains(PASSWORD));
    assert!(stderr.contains("remote signing on"), "{stderr}");
    assert_only_info(&seen, 1);
}

/// Without `--remote-signing` the MCP server's tools never reach the network: the call fails
/// saying so, and the service hears nothing.
#[cfg(feature = "mcp")]
#[test]
fn mcp_stays_off_the_network_without_remote_signing() {
    let (url, seen) = oauth_only_service();
    let (reply, _, stderr) = mcp_sign_remote_ids(&[], &url);
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    assert_eq!(reply["result"]["content"][0]["text"], "remote signing needs network access, which this host did not enable", "{reply}");
    assert!(!stderr.contains("remote signing on"), "{stderr}");
    assert_only_info(&seen, 0);
}

/// The tools `mcp` with `args` lists as open-world (`openWorldHint`).
#[cfg(feature = "mcp")]
fn mcp_open_world(args: &[&str]) -> Vec<String> {
    use std::process::Stdio;
    let mut child = Command::new(BIN).arg("mcp").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let input = "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{}}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n";
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let reply: serde_json::Value = serde_json::from_str(stdout.lines().nth(1).unwrap()).unwrap();
    let tools = reply["result"]["tools"].as_array().unwrap();
    tools.iter().filter(|t| t["annotations"]["openWorldHint"] == true).map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

/// `mcp` tells its client a tool reaches past this computer only when it can: with
/// `--remote-signing`, the remote signing tools (or, compact, `tool_call`, which runs them);
/// without it, none, local signing with `sign_document` included.
#[cfg(feature = "mcp")]
#[test]
fn mcp_marks_tools_open_world_only_with_remote_signing() {
    assert_eq!(mcp_open_world(&["--remote-signing"]), ["sign_document", "sign_remote_ids", "sign_remote_send_otp"]);
    assert_eq!(mcp_open_world(&["--remote-signing", "--compact"]), ["tool_call"]);
    assert!(mcp_open_world(&[]).is_empty());
    assert!(mcp_open_world(&["--compact"]).is_empty());
}

/// `tools` says which tools reach past this computer given a network (`run` has one), and that
/// listing remote certificates is not read-only (a refused sign-in counts towards locking the
/// account).
#[test]
fn tools_lists_the_open_world_ones() {
    let out = Command::new(BIN).arg("tools").output().unwrap();
    let tools: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    let open: Vec<&str> = tools.iter().filter(|t| t["open_world"] == true).map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(open, ["sign_document", "sign_remote_ids", "sign_remote_send_otp"]);
    assert!(tools.iter().filter(|t| t["open_world"] == true).all(|t| t["read_only"] == false));
}
