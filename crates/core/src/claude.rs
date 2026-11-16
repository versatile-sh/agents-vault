use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::http::get_json;
use crate::launch::harness_command;
use crate::limits::{Limits, Window};
use crate::model::Account;
use crate::proc;
use crate::store::{now, Store};

const USAGE: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE: &str = "https://api.anthropic.com/api/oauth/profile";
const BETA: &str = "oauth-2025-04-20";

/// Access token Claude Code saved for an account, read only and never stored elsewhere.
pub struct Token {
    pub access: String,
    pub expires_at: i64,
}

/// Reads the saved access token of a Claude Code home, if it keeps one on disk.
pub fn token(home: &Path) -> Option<Token> {
    let raw = std::fs::read_to_string(home.join(".credentials.json")).ok()?;
    let json: Value = serde_json::from_str(&raw).ok()?;
    let login = json.get("claudeAiOauth")?;
    let access = login
        .get("accessToken")?
        .as_str()
        .filter(|token| !token.is_empty())?;
    let expires_at = login.get("expiresAt").and_then(Value::as_i64).unwrap_or(0) / 1000;
    Some(Token {
        access: access.to_string(),
        expires_at,
    })
}

/// Token that is still valid, asking Claude Code to renew its own session first when it lapsed.
pub fn fresh_token(store: &Store, account: &Account) -> Result<Token> {
    let home = store.home(account);
    let live = |token: &Token| token.expires_at == 0 || token.expires_at > now() + 60;
    let saved = token(&home).context("Claude Code keeps no sign-in on disk for this account")?;
    if live(&saved) {
        return Ok(saved);
    }
    let mut command = harness_command(store, account)?;
    command.args(["auth", "status"]);
    proc::run(command, Duration::from_secs(120))?;
    token(&home)
        .filter(live)
        .context("the sign-in expired; sign in again")
}

fn get(url: &str, token: &str) -> Result<Value> {
    let authorization = format!("Bearer {token}");
    get_json(
        url,
        &[("Authorization", &authorization), ("anthropic-beta", BETA)],
    )
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468
}

/// Parses an RFC 3339 timestamp into Unix seconds.
pub fn parse_time(text: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (date, rest) = (text.get(..19)?, text.get(19..)?);
    let bytes = date.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' || !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    let seconds = days_from_civil(number(0..4)?, number(5..7)?, number(8..10)?) * 86_400
        + number(11..13)? * 3600
        + number(14..16)? * 60
        + number(17..19)?;
    let zone = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match zone.as_bytes().first() {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..6).and_then(|m| m.parse().ok()).unwrap_or(0);
            (hours * 3600 + minutes * 60) * if *sign == b'-' { -1 } else { 1 }
        }
        _ => return None,
    };
    Some(seconds - offset)
}

fn window(payload: &Value, names: &[&str]) -> Option<Window> {
    let bucket = names
        .iter()
        .filter_map(|name| payload.get(name))
        .find(|bucket| bucket.is_object())?;
    let percent = bucket
        .get("utilization")?
        .as_f64()
        .filter(|value| value.is_finite())?;
    let resets_at = bucket
        .get("resets_at")
        .and_then(Value::as_str)
        .and_then(parse_time);
    Some(Window {
        percent: percent.clamp(0.0, 100.0),
        resets_at,
    })
}

fn parse_usage(payload: &Value) -> Limits {
    let idle = || {
        Some(Window {
            percent: 0.0,
            resets_at: None,
        })
    };
    Limits {
        session: window(payload, &["five_hour"]).or_else(idle),
        weekly: window(payload, &["seven_day_oauth_apps", "seven_day"]).or_else(idle),
        captured_at: now(),
    }
}

/// Asks Anthropic for the 5-hour and weekly usage of the signed-in account.
pub fn usage(token: &str) -> Result<Limits> {
    let payload = get(USAGE, token)?;
    if !payload.is_object() {
        bail!("the usage endpoint returned no limits");
    }
    Ok(parse_usage(&payload))
}

fn parse_plan(payload: &Value) -> Option<String> {
    let organization = payload.get("organization")?;
    ["plan_display_name", "plan_display_label"]
        .iter()
        .filter_map(|key| organization.get(key)?.as_str())
        .map(str::trim)
        .find(|name| !name.is_empty())
        .map(str::to_string)
}

/// Asks Anthropic for the plan name shown to the signed-in account.
pub fn plan(token: &str) -> Result<Option<String>> {
    Ok(parse_plan(&get(PROFILE, token)?))
}

/// Turns a tier id such as `max_20x` into the label people read: `Max 20x`.
pub fn tier_label(tier: &str) -> String {
    let words: Vec<String> = tier
        .trim_start_matches("default_claude_")
        .split('_')
        .filter(|word| !word.is_empty())
        .enumerate()
        .map(|(index, word)| {
            let mut letters = word.chars();
            match (index, letters.next()) {
                (0, Some(first)) => first.to_uppercase().chain(letters).collect(),
                _ => word.to_string(),
            }
        })
        .collect();
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_timestamps_with_fractions_and_offsets() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_time("2026-11-18T04:59:59.940912+00:00"),
            Some(1_794_977_999)
        );
        assert_eq!(parse_time("2026-11-18T01:59:59-03:00"), Some(1_794_977_999));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_time("2024-02-29T12:00:00Z"), Some(1_709_208_000));
        assert_eq!(parse_time("soon"), None);
    }

    #[test]
    fn reads_usage_and_treats_an_absent_window_as_unused() {
        let limits = parse_usage(&json!({
            "five_hour": null,
            "seven_day": { "utilization": 46.0, "resets_at": "2026-11-18T04:59:59.940912+00:00" }
        }));
        assert_eq!(
            limits.session,
            Some(Window {
                percent: 0.0,
                resets_at: None
            })
        );
        assert_eq!(
            limits.weekly,
            Some(Window {
                percent: 46.0,
                resets_at: Some(1_794_977_999)
            })
        );
    }

    #[test]
    fn prefers_the_display_name_of_the_plan() {
        let payload = json!({ "organization": { "plan_display_name": " Ultra ", "plan_display_label": "x" } });
        assert_eq!(parse_plan(&payload).as_deref(), Some("Ultra"));
        assert_eq!(
            parse_plan(&json!({ "organization": { "plan_display_name": "" } })),
            None
        );
    }

    #[test]
    fn labels_a_tier_id() {
        assert_eq!(tier_label("max_20x"), "Max 20x");
        assert_eq!(tier_label("default_claude_max_5x"), "Max 5x");
        assert_eq!(tier_label("pro"), "Pro");
    }

    #[test]
    fn reads_only_a_present_token() {
        let directory = tempfile::tempdir().unwrap();
        assert!(token(directory.path()).is_none());
        std::fs::write(
            directory.path().join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":1700000000000}}"#,
        )
        .unwrap();
        let saved = token(directory.path()).unwrap();
        assert_eq!(
            (saved.access.as_str(), saved.expires_at),
            ("abc", 1_700_000_000)
        );
    }
}
