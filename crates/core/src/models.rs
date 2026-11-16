use std::path::PathBuf;

use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::codex;
use crate::http::{get_json, join};
use crate::model::{Account, Auth, Harness, Provider};
use crate::secrets;
use crate::store::Store;

const ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com";
const OPENAI_ENDPOINT: &str = "https://api.openai.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// A model an account can start a session on.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
}

fn option(id: &str, label: &str) -> ModelOption {
    ModelOption { id: id.to_string(), label: label.to_string() }
}

/// Model aliases Claude Code documents for every subscription.
pub fn claude_aliases() -> Vec<ModelOption> {
    vec![
        option("fable", "Fable"),
        option("opus", "Opus"),
        option("sonnet", "Sonnet"),
        option("haiku", "Haiku"),
        option("opus[1m]", "Opus · 1M context"),
        option("sonnet[1m]", "Sonnet · 1M context"),
        option("opusplan", "Opus to plan, Sonnet to build"),
    ]
}

fn claude_state(store: &Store, account: &Account) -> Option<PathBuf> {
    let home = store.home(account);
    if account.native_home {
        return Some(home.parent()?.join(".claude.json"));
    }
    Some(home.join(".claude.json"))
}

fn claude_cached(store: &Store, account: &Account) -> Vec<ModelOption> {
    let state: Option<Value> = claude_state(store, account)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok());
    let entries = state.as_ref().and_then(|state| state.get("additionalModelOptionsCache")?.as_array().cloned());
    entries
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            let id = entry.get("value")?.as_str()?;
            let label = entry.get("description").or(entry.get("label")).and_then(Value::as_str).unwrap_or(id);
            Some(option(id, label.split(" · ").next().unwrap_or(label)))
        })
        .collect()
}

fn parse_codex(result: &Value) -> Vec<ModelOption> {
    let entries = result.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
    entries
        .iter()
        .filter(|entry| entry.get("hidden").and_then(Value::as_bool) != Some(true))
        .filter_map(|entry| {
            let id = entry.get("model").or(entry.get("id"))?.as_str()?;
            Some(option(id, entry.get("displayName").and_then(Value::as_str).unwrap_or(id)))
        })
        .collect()
}

fn parse_listing(payload: &Value) -> Vec<ModelOption> {
    let entries = payload.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
    entries
        .iter()
        .filter_map(|entry| {
            let id = entry.get("id")?.as_str()?;
            let label = entry.get("display_name").or(entry.get("name")).and_then(Value::as_str).unwrap_or(id);
            Some(option(id, label))
        })
        .take(1000)
        .collect()
}

/// Asks an API endpoint which models a key can use.
pub fn remote(harness: Harness, base_url: Option<&str>, bearer: bool, key: &str) -> Result<Vec<ModelOption>> {
    let authorization = format!("Bearer {key}");
    let payload = match harness {
        Harness::Claude => {
            let url = join(base_url.unwrap_or(ANTHROPIC_ENDPOINT), "v1/models?limit=1000");
            let credential = if bearer { ("Authorization", authorization.as_str()) } else { ("x-api-key", key) };
            get_json(&url, &[credential, ("anthropic-version", ANTHROPIC_VERSION)])?
        }
        Harness::Codex => get_json(&join(base_url.unwrap_or(OPENAI_ENDPOINT), "models"), &[("Authorization", &authorization)])?,
    };
    let models = parse_listing(&payload);
    if models.is_empty() {
        bail!("the endpoint listed no models");
    }
    Ok(models)
}

/// Models an existing account can start on, from the source that knows best.
pub fn for_account(store: &Store, account: &Account) -> Result<Vec<ModelOption>> {
    match (&account.auth, account.harness) {
        (Auth::Login, Harness::Claude) => {
            let mut models = claude_aliases();
            for extra in claude_cached(store, account) {
                if !models.iter().any(|known| known.id == extra.id) {
                    models.push(extra);
                }
            }
            Ok(models)
        }
        (Auth::Login, Harness::Codex) => Ok(parse_codex(&codex::request(store, account, "model/list", json!({}))?)),
        (Auth::ApiKey { base_url, bearer }, harness) => {
            let key = secrets::get(&account.id)?.ok_or_else(|| anyhow::anyhow!("no API key saved for this account"))?;
            remote(harness, base_url.as_deref(), *bearer, &key)
        }
    }
}

/// Models to offer before an account exists, taken from the harness default home.
pub fn for_harness(store: &Store, harness: Harness) -> Result<Vec<ModelOption>> {
    let mut account = Account::new("default", harness, Auth::Login)?;
    account.native_home = true;
    account.home = harness.native_home();
    match harness {
        Harness::Claude => Ok(claude_aliases()),
        Harness::Codex => for_account(store, &account),
    }
}

/// Models a provider key can use, asked from the provider itself.
pub fn for_provider(provider: &Provider, key: &str) -> Result<Vec<ModelOption>> {
    match (&provider.openai_url, &provider.anthropic_url) {
        (Some(url), _) => remote(Harness::Codex, Some(url), true, key),
        (None, Some(url)) => remote(Harness::Claude, Some(url), true, key),
        (None, None) => bail!("this provider has no endpoint to ask"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_visible_codex_models_by_their_display_name() {
        let models = parse_codex(&json!({ "data": [
            { "id": "a", "model": "gpt-a", "displayName": "GPT A", "hidden": false },
            { "id": "b", "model": "gpt-b", "displayName": "GPT B", "hidden": true }
        ]}));
        assert_eq!(models, vec![option("gpt-a", "GPT A")]);
    }

    #[test]
    fn reads_api_listings_from_either_naming() {
        let models = parse_listing(&json!({ "data": [
            { "id": "claude-x", "display_name": "Claude X" },
            { "id": "vendor/y", "name": "Vendor: Y" },
            { "id": "bare" },
            { "name": "no id" }
        ]}));
        assert_eq!(models, vec![option("claude-x", "Claude X"), option("vendor/y", "Vendor: Y"), option("bare", "bare")]);
    }

    #[test]
    fn claude_sign_in_offers_aliases_plus_the_options_cached_for_the_account() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::at(directory.path().to_path_buf()).unwrap();
        let account = Account::new("work", Harness::Claude, Auth::Login).unwrap();
        let home = store.ensure_home(&account).unwrap();
        std::fs::write(
            home.join(".claude.json"),
            r#"{"additionalModelOptionsCache":[{"value":"claude-fable-5-2[1m]","label":"Fable","description":"Fable 5.2 · Most capable"},{"value":"opus","label":"Opus"}]}"#,
        )
        .unwrap();
        let models = for_account(&store, &account).unwrap();
        assert_eq!(models.len(), claude_aliases().len() + 1);
        assert_eq!(models.last(), Some(&option("claude-fable-5-2[1m]", "Fable 5.2")));
    }
}
