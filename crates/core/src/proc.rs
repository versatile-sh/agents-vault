use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

/// Captured result of a finished helper process.
pub struct Output {
    pub success: bool,
    pub stdout: String,
}

/// Runs a command to completion, killing it when the deadline passes.
pub fn run(mut command: Command, timeout: Duration) -> Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("could not start the harness CLI")?;
    let mut pipe = child.stdout.take().context("missing stdout pipe")?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = pipe.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the harness CLI did not answer in {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    let stdout = reader.join().unwrap_or_default();
    Ok(Output {
        success: status.success(),
        stdout,
    })
}
