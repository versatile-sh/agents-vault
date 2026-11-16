use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

/// Fetches JSON over HTTPS with the given headers; redirects are not followed.
pub fn get_json(url: &str, headers: &[(&str, &str)]) -> Result<Value> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .max_redirects(0)
        .build()
        .into();
    let mut request = agent.get(url).header("Accept", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request.call().map_err(|error| anyhow::anyhow!("{} did not answer: {error}", host(url)))?;
    response.body_mut().read_json::<Value>().with_context(|| format!("{} sent an unreadable answer", host(url)))
}

/// Host part of a URL, for messages and provider detection.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', ':', '?']).next().unwrap_or(rest)
}

/// Joins a base endpoint and a path without doubling the slash.
pub fn join(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_hosts_and_joins_paths() {
        assert_eq!(host("https://openrouter.ai/api/v1"), "openrouter.ai");
        assert_eq!(host("http://localhost:4000"), "localhost");
        assert_eq!(join("https://api.deepseek.com/", "/models"), "https://api.deepseek.com/models");
    }
}
