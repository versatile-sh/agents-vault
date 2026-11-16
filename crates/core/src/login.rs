use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};

use crate::kit;
use crate::launch::harness_command;
use crate::model::{Account, Config, Harness};
use crate::store::Store;

/// A harness sign-in running in the background, with no terminal attached.
pub struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    url: Arc<Mutex<Option<String>>>,
}

/// First https URL in a line of harness output, without terminal escape codes.
pub fn find_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let url: String = line[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && !c.is_control())
        .collect();
    Some(url).filter(|url| url.len() > "https://".len())
}

/// Starts the official sign-in of the harness; it opens the browser and finishes on its own.
pub fn start(store: &Store, config: &Config, account: &Account) -> Result<Session> {
    if !account.is_login() {
        bail!("`{}` uses an API key and has no sign-in", account.label);
    }
    kit::apply(store, config, account)?;
    let mut command = harness_command(store, account)?;
    match account.harness {
        Harness::Claude => command.args(["auth", "login"]),
        Harness::Codex | Harness::Grok => command.arg("login"),
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().context("could not start the sign-in")?;
    let stdout = child.stdout.take().context("missing stdout pipe")?;
    let url = Arc::new(Mutex::new(None));
    let found = Arc::clone(&url);
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
            if let (Some(url), Ok(mut slot)) = (find_url(&line), found.lock()) {
                slot.get_or_insert(url);
            }
        }
    });
    Ok(Session {
        stdin: child.stdin.take(),
        child,
        url,
    })
}

impl Session {
    /// Sign-in page printed by the harness, for opening it again by hand.
    pub fn url(&self) -> Option<String> {
        self.url.lock().ok()?.clone()
    }

    /// Sends the code shown by the sign-in page when the browser could not hand it over.
    pub fn send_code(&mut self, code: &str) -> Result<()> {
        let code = code.trim();
        if code.is_empty() || code.chars().any(char::is_control) {
            bail!("paste the code shown on the sign-in page");
        }
        let stdin = self
            .stdin
            .as_mut()
            .context("the sign-in is no longer waiting for a code")?;
        writeln!(stdin, "{code}")?;
        stdin.flush()?;
        Ok(())
    }

    /// `Some(true)` when the harness finished signing in, `Some(false)` when it gave up.
    pub fn finished(&mut self) -> Option<bool> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.success()),
            Ok(None) => None,
            Err(_) => Some(false),
        }
    }

    /// Stops the sign-in.
    pub fn cancel(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.finished().is_none() {
            self.cancel();
        }
    }
}

/// Opens a sign-in page in the default browser.
pub fn open_browser(url: &str) -> Result<()> {
    if !url.starts_with("https://") || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("refusing to open something that is not an https page");
    }
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
        .context("could not open the browser")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_sign_in_page_inside_a_terminal_hyperlink() {
        let line = "If the browser didn't open, visit: \u{1b}]8;;https://claude.com/cai/oauth/authorize?code=true&state=abc\u{1b}\\https://claude.com/x";
        assert_eq!(
            find_url(line).as_deref(),
            Some("https://claude.com/cai/oauth/authorize?code=true&state=abc")
        );
        assert_eq!(
            find_url("Starting local login server on http://localhost:1455."),
            None
        );
        assert_eq!(find_url("https://"), None);
    }

    #[test]
    fn opens_only_https_pages() {
        assert!(open_browser("file:///etc/passwd").is_err());
        assert!(open_browser("https://example.com/a b").is_err());
    }
}
