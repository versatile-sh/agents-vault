use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::launch::harness_command;
use crate::model::Account;
use crate::store::Store;

/// Calls one method on the Codex app-server of an account and returns its result.
pub fn request(store: &Store, account: &Account, method: &str, params: Value) -> Result<Value> {
    let mut command = harness_command(store, account)?;
    command
        .args(["-s", "read-only", "-a", "never", "app-server"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().context("could not start codex")?;
    let mut stdin = child.stdin.take().context("missing stdin pipe")?;
    let stdout = child.stdout.take().context("missing stdout pipe")?;
    let (sender, receiver) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
            if let Ok(message) = serde_json::from_str(&line) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        }
    });
    let mut send = |message: Value, id: Option<i64>, seconds: u64| -> Result<Value> {
        writeln!(stdin, "{message}")?;
        stdin.flush()?;
        let Some(id) = id else { return Ok(Value::Null) };
        loop {
            let reply = receiver
                .recv_timeout(Duration::from_secs(seconds))
                .context("codex did not answer in time")?;
            if reply.get("id").and_then(Value::as_i64) == Some(id) {
                if let Some(error) = reply.get("error") {
                    bail!("codex refused the request: {error}");
                }
                return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    };
    let info = json!({ "clientInfo": { "name": "vvault", "version": env!("CARGO_PKG_VERSION") } });
    let outcome = send(json!({ "id": 1, "method": "initialize", "params": info }), Some(1), 10)
        .and_then(|_| send(json!({ "method": "initialized", "params": {} }), None, 0))
        .and_then(|_| send(json!({ "id": 2, "method": method, "params": params }), Some(2), 15));
    let _ = child.kill();
    let _ = child.wait();
    outcome
}
