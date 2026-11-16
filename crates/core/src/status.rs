use std::fs::File;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::claude;
use crate::codex;
use crate::launch::harness_command;
use crate::limits;
use crate::model::{Account, Config, Harness};
use crate::proc;
use crate::store::{now, read_json, write_json, Store};

const FRESH_SECONDS: i64 = 50 * 60;

/// Last observed session state of a login account.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Status {
    pub logged_in: bool,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub checked_at: i64,
}

/// Reads the cached session state of an account.
pub fn read(store: &Store, id: &str) -> Option<Status> {
    read_json(&store.cache("status", id, "json").ok()?)
}

fn parse_claude(stdout: &str) -> Status {
    let json: Value = stdout
        .find('{')
        .and_then(|start| serde_json::from_str(&stdout[start..]).ok())
        .unwrap_or(Value::Null);
    let text = |key: &str| json.get(key).and_then(Value::as_str).map(str::to_string);
    Status {
        logged_in: json.get("loggedIn").and_then(Value::as_bool) == Some(true),
        email: text("email"),
        plan: text("planTier")
            .map(|tier| claude::tier_label(&tier))
            .or_else(|| text("subscriptionType")),
        checked_at: now(),
    }
}

fn parse_codex(reply: &Value) -> Status {
    let account = reply.get("account").filter(|account| account.is_object());
    let text = |key: &str| account?.get(key)?.as_str().map(str::to_string);
    Status {
        logged_in: account.is_some(),
        email: text("email"),
        plan: text("planType").map(|plan| claude::tier_label(&plan)),
        checked_at: now(),
    }
}

fn claude_plan(store: &Store, account: &Account) -> Option<String> {
    let token = claude::token(&store.home(account))?;
    claude::plan(&token.access).ok().flatten()
}

/// Asks the harness CLI for the session state; the CLI renews its own token.
pub fn check(store: &Store, account: &Account) -> Result<Status> {
    let mut command = harness_command(store, account)?;
    let status = match account.harness {
        Harness::Claude => {
            command.args(["auth", "status"]);
            let mut status = parse_claude(&proc::run(command, Duration::from_secs(120))?.stdout);
            if status.logged_in {
                status.plan = claude_plan(store, account).or(status.plan);
            }
            status
        }
        Harness::Codex => {
            let reply = codex::request(
                store,
                account,
                "account/read",
                json!({ "refreshToken": false }),
            )?;
            let _ = limits::probe_codex(store, account);
            parse_codex(&reply)
        }
        Harness::Grok => {
            command.arg("models");
            let output = proc::run(command, Duration::from_secs(60))?;
            Status {
                logged_in: output.success && !output.stdout.contains("not authenticated"),
                checked_at: now(),
                ..Status::default()
            }
        }
    };
    write_json(&store.cache("status", &account.id, "json")?, &status)?;
    Ok(status)
}

/// Outcome of one keep-alive pass over an account.
pub enum Pass {
    Checked(Status),
    Fresh,
    Busy,
    Failed(String),
}

/// Renews every login account that is due, one process at a time per account.
pub fn keepalive(store: &Store, config: &Config, force: bool) -> Vec<(Account, Pass)> {
    let due = |account: &Account| {
        force
            || read(store, &account.id)
                .is_none_or(|status| now() - status.checked_at >= FRESH_SECONDS)
    };
    let pass = |account: &Account| -> Result<Pass> {
        if !due(account) {
            return Ok(Pass::Fresh);
        }
        let lock = File::options().create(true).append(true).open(store.cache(
            "status",
            &account.id,
            "lock",
        )?)?;
        if lock.try_lock().is_err() {
            return Ok(Pass::Busy);
        }
        if !due(account) {
            return Ok(Pass::Fresh);
        }
        check(store, account).map(Pass::Checked)
    };
    std::thread::scope(|scope| {
        let handles: Vec<_> = config
            .accounts
            .iter()
            .filter(|account| account.is_login())
            .map(|account| {
                let handle = scope.spawn(move || {
                    pass(account).unwrap_or_else(|error| Pass::Failed(format!("{error:#}")))
                });
                (account.clone(), handle)
            })
            .collect();
        handles
            .into_iter()
            .map(|(account, handle)| {
                let pass = handle
                    .join()
                    .unwrap_or_else(|_| Pass::Failed("keep-alive crashed".into()));
                (account, pass)
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_login_state_after_wrapper_noise() {
        let status = parse_claude("mise: installing\n{\"loggedIn\":true,\"email\":\"a@b.c\",\"subscriptionType\":\"max\"}");
        assert!(status.logged_in);
        assert_eq!(status.email.as_deref(), Some("a@b.c"));
        assert_eq!(status.plan.as_deref(), Some("max"));
        let tiered = parse_claude(
            "{\"loggedIn\":true,\"subscriptionType\":\"max\",\"planTier\":\"max_20x\"}",
        );
        assert_eq!(tiered.plan.as_deref(), Some("Max 20x"));
    }

    #[test]
    fn treats_unreadable_output_as_logged_out() {
        assert!(!parse_claude("boom").logged_in);
        assert!(!parse_claude("{\"loggedIn\":false}").logged_in);
    }

    #[test]
    fn reads_codex_account_and_treats_no_account_as_signed_out() {
        let status = parse_codex(
            &json!({ "account": { "type": "chatgpt", "email": "a@b.c", "planType": "plus" } }),
        );
        assert!(status.logged_in);
        assert_eq!(
            (status.email.as_deref(), status.plan.as_deref()),
            (Some("a@b.c"), Some("Plus"))
        );
        assert!(!parse_codex(&json!({ "account": null })).logged_in);
    }
}
