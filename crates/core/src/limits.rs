use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::claude;
use crate::codex;
use crate::model::{Account, Harness};
use crate::store::{now, read_json, write_json, Store};

const WEEK_MINUTES: i64 = 10_080;
const MIN_PROBE_GAP: i64 = 15;

/// Usage of one rate-limit window, as a 0–100 percentage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub percent: f64,
    pub resets_at: Option<i64>,
}

/// Last known subscription limits of an account.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub session: Option<Window>,
    pub weekly: Option<Window>,
    pub captured_at: i64,
}

impl Limits {
    /// Zeroes every window whose reset time has already passed.
    pub fn settled(mut self, now: i64) -> Self {
        for window in [&mut self.session, &mut self.weekly].into_iter().flatten() {
            if window.resets_at.is_some_and(|at| at <= now) {
                *window = Window {
                    percent: 0.0,
                    resets_at: None,
                };
            }
        }
        self
    }
}

fn window(percent: Option<f64>, resets_at: Option<i64>) -> Option<Window> {
    let percent = percent.filter(|value| value.is_finite())?;
    Some(Window {
        percent: percent.clamp(0.0, 100.0),
        resets_at: resets_at.filter(|at| *at > 0),
    })
}

/// Reads the cached limits of an account with expired windows reset.
pub fn read(store: &Store, id: &str) -> Option<Limits> {
    let path = store.cache("limits", id, "json").ok()?;
    read_json::<Limits>(&path).map(|limits| limits.settled(now()))
}

fn save(store: &Store, id: &str, limits: &Limits) -> Result<()> {
    write_json(&store.cache("limits", id, "json")?, limits)
}

/// Extracts `rate_limits` from Claude Code status line input and caches it.
pub fn capture_claude(store: &Store, id: &str, input: &Value) -> Result<Option<Limits>> {
    let read_window = |name: &str| {
        let entry = input.get("rate_limits")?.get(name)?;
        window(
            entry.get("used_percentage")?.as_f64(),
            entry.get("resets_at").and_then(Value::as_i64),
        )
    };
    let (session, weekly) = (read_window("five_hour"), read_window("seven_day"));
    if session.is_none() && weekly.is_none() {
        return Ok(None);
    }
    let previous = read(store, id).unwrap_or_default();
    let limits = Limits {
        session: session.or(previous.session),
        weekly: weekly.or(previous.weekly),
        captured_at: now(),
    };
    save(store, id, &limits)?;
    Ok(Some(limits))
}

fn codex_window(entry: &Value) -> Option<(bool, Window)> {
    let window = window(
        entry.get("usedPercent")?.as_f64(),
        entry.get("resetsAt").and_then(Value::as_i64),
    )?;
    let weekly = entry.get("windowDurationMins").and_then(Value::as_i64) == Some(WEEK_MINUTES);
    Some((weekly, window))
}

fn parse_codex(result: &Value) -> Limits {
    let mut limits = Limits {
        captured_at: now(),
        ..Limits::default()
    };
    for name in ["primary", "secondary"] {
        match result
            .pointer(&format!("/rateLimits/{name}"))
            .and_then(codex_window)
        {
            Some((true, window)) => limits.weekly = Some(window),
            Some((false, window)) => limits.session = Some(window),
            None => {}
        }
    }
    limits
}

/// Asks the Codex CLI itself for the account limits and caches the answer.
pub fn probe_codex(store: &Store, account: &Account) -> Result<Limits> {
    let result = codex::request(store, account, "account/rateLimits/read", Value::Null)?;
    let limits = parse_codex(&result);
    if limits.session.is_none() && limits.weekly.is_none() {
        bail!("codex returned no rate limits for this account");
    }
    save(store, &account.id, &limits)?;
    Ok(limits)
}

/// Asks Anthropic for the limits of a Claude Code account, using the token Claude Code saved.
pub fn probe_claude(store: &Store, account: &Account) -> Result<Limits> {
    let token = claude::fresh_token(store, account)?;
    let limits = claude::usage(&token.access)?;
    save(store, &account.id, &limits)?;
    Ok(limits)
}

/// Best known limits: cached when fresh enough, otherwise asked from the provider.
pub fn current(store: &Store, account: &Account, max_age: i64) -> Option<Limits> {
    if !account.is_login() {
        return None;
    }
    let cached = read(store, &account.id);
    if cached
        .as_ref()
        .is_some_and(|limits| now() - limits.captured_at < max_age.max(MIN_PROBE_GAP))
    {
        return cached;
    }
    let probed = match account.harness {
        Harness::Claude => probe_claude(store, account),
        Harness::Codex => probe_codex(store, account),
        Harness::Grok => return None,
    };
    probed.ok().or(cached)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "0b0e7a52-5d0c-4b62-9d43-0d2f6a0f5a11";

    fn store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::at(directory.path().to_path_buf()).unwrap();
        (directory, store)
    }

    #[test]
    fn captures_both_windows_from_status_line_input() {
        let (_guard, store) = store();
        let later = now() + 3600;
        let input = json!({ "rate_limits": {
            "five_hour": { "used_percentage": 23.5, "resets_at": later },
            "seven_day": { "used_percentage": 141.0, "resets_at": later }
        }});
        capture_claude(&store, ID, &input).unwrap().unwrap();
        let limits = read(&store, ID).unwrap();
        assert_eq!(
            limits.session,
            Some(Window {
                percent: 23.5,
                resets_at: Some(later)
            })
        );
        assert_eq!(limits.weekly.unwrap().percent, 100.0);
    }

    #[test]
    fn keeps_the_last_known_window_when_one_is_missing() {
        let (_guard, store) = store();
        let later = now() + 3600;
        let full = json!({ "rate_limits": {
            "five_hour": { "used_percentage": 10, "resets_at": later },
            "seven_day": { "used_percentage": 40, "resets_at": later }
        }});
        capture_claude(&store, ID, &full).unwrap();
        let partial = json!({ "rate_limits": { "five_hour": { "used_percentage": 12, "resets_at": later } } });
        let limits = capture_claude(&store, ID, &partial).unwrap().unwrap();
        assert_eq!(limits.session.unwrap().percent, 12.0);
        assert_eq!(limits.weekly.unwrap().percent, 40.0);
    }

    #[test]
    fn ignores_input_without_rate_limits() {
        let (_guard, store) = store();
        assert!(capture_claude(&store, ID, &json!({ "model": {} }))
            .unwrap()
            .is_none());
        assert!(read(&store, ID).is_none());
    }

    #[test]
    fn resets_a_window_once_its_reset_time_passes() {
        let limits = Limits {
            session: Some(Window {
                percent: 90.0,
                resets_at: Some(100),
            }),
            weekly: Some(Window {
                percent: 50.0,
                resets_at: Some(300),
            }),
            captured_at: 50,
        }
        .settled(200);
        assert_eq!(
            limits.session,
            Some(Window {
                percent: 0.0,
                resets_at: None
            })
        );
        assert_eq!(limits.weekly.unwrap().percent, 50.0);
    }

    #[test]
    fn sorts_codex_windows_by_duration() {
        let limits = parse_codex(&json!({ "rateLimits": {
            "primary": { "usedPercent": 7, "windowDurationMins": 300, "resetsAt": 9_999_999_999i64 },
            "secondary": { "usedPercent": 61, "windowDurationMins": 10080, "resetsAt": 9_999_999_999i64 }
        }}));
        assert_eq!(limits.session.unwrap().percent, 7.0);
        assert_eq!(limits.weekly.unwrap().percent, 61.0);
    }
}
