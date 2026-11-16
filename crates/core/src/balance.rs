use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http::{get_json, host};
use crate::store::{now, read_json, write_json, Store};

const FRESH_SECONDS: i64 = 120;

/// What a provider reports about the money behind a key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Balance {
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    pub currency: String,
    pub captured_at: i64,
}

fn parse_openrouter(payload: &Value) -> Option<Balance> {
    let data = payload.get("data")?;
    let number = |key: &str| data.get(key).and_then(Value::as_f64);
    Some(Balance {
        used: number("usage"),
        limit: number("limit"),
        remaining: number("limit_remaining"),
        currency: "USD".into(),
        captured_at: now(),
    })
}

fn parse_deepseek(payload: &Value) -> Option<Balance> {
    let infos = payload.get("balance_infos")?.as_array()?;
    let info = infos
        .iter()
        .find(|info| info.get("currency").and_then(Value::as_str) == Some("USD"))
        .or(infos.first())?;
    let total = info.get("total_balance")?;
    let remaining = total.as_f64().or_else(|| total.as_str()?.parse().ok())?;
    Some(Balance {
        remaining: Some(remaining),
        currency: info
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("USD")
            .to_string(),
        captured_at: now(),
        ..Balance::default()
    })
}

/// True when the provider behind an endpoint has a balance request Vault knows.
pub fn supported(endpoint: &str) -> bool {
    matches!(host(endpoint), "openrouter.ai" | "api.deepseek.com")
}

fn fetch(endpoint: &str, key: &str) -> Result<Option<Balance>> {
    let authorization = format!("Bearer {key}");
    let headers = [("Authorization", authorization.as_str())];
    Ok(match host(endpoint) {
        "openrouter.ai" => {
            parse_openrouter(&get_json("https://openrouter.ai/api/v1/key", &headers)?)
        }
        "api.deepseek.com" => parse_deepseek(&get_json(
            "https://api.deepseek.com/user/balance",
            &headers,
        )?),
        _ => None,
    })
}

/// Balance of a key, cached briefly; `None` for providers that do not report one.
pub fn current(store: &Store, id: &str, endpoint: Option<&str>, max_age: i64) -> Option<Balance> {
    let endpoint = endpoint.filter(|endpoint| supported(endpoint))?;
    let path = store.cache("balance", id, "json").ok()?;
    let cached: Option<Balance> = read_json(&path);
    if cached
        .as_ref()
        .is_some_and(|balance| now() - balance.captured_at < max_age.max(FRESH_SECONDS))
    {
        return cached;
    }
    let key = crate::secrets::get(id).ok().flatten()?;
    match fetch(endpoint, &key) {
        Ok(Some(balance)) => {
            let _ = write_json(&path, &balance);
            Some(balance)
        }
        _ => cached,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_openrouter_key_spending() {
        let balance = parse_openrouter(
            &json!({ "data": { "usage": 12.5, "limit": 50, "limit_remaining": 37.5 } }),
        )
        .unwrap();
        assert_eq!(
            (balance.used, balance.limit, balance.remaining),
            (Some(12.5), Some(50.0), Some(37.5))
        );
        let open = parse_openrouter(
            &json!({ "data": { "usage": 3.0, "limit": null, "limit_remaining": null } }),
        )
        .unwrap();
        assert_eq!((open.used, open.limit), (Some(3.0), None));
    }

    #[test]
    fn reads_deepseek_balance_strings_preferring_dollars() {
        let balance = parse_deepseek(&json!({ "is_available": true, "balance_infos": [
            { "currency": "CNY", "total_balance": "700.00" },
            { "currency": "USD", "total_balance": "110.00" }
        ]}))
        .unwrap();
        assert_eq!(
            (balance.remaining, balance.currency.as_str()),
            (Some(110.0), "USD")
        );
    }

    #[test]
    fn only_known_providers_are_asked() {
        assert!(supported("https://openrouter.ai/api/v1"));
        assert!(supported("https://api.deepseek.com/anthropic"));
        assert!(!supported("https://gateway.example/v1"));
        assert!(!supported("https://openrouter.ai.evil.example/v1"));
    }
}
