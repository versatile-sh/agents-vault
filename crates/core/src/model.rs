use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Coding agent CLI an account launches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
    Grok,
}

impl Harness {
    /// Executable name looked up on PATH.
    pub fn program(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Grok => "grok",
        }
    }

    /// Environment variable that relocates the harness home directory.
    pub fn home_env(self) -> &'static str {
        match self {
            Harness::Claude => "CLAUDE_CONFIG_DIR",
            Harness::Codex => "CODEX_HOME",
            Harness::Grok => "GROK_HOME",
        }
    }

    /// Home directory the harness uses when no override is set.
    pub fn native_home(self) -> Option<PathBuf> {
        let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
        Some(home.join(match self {
            Harness::Claude => ".claude",
            Harness::Codex => ".codex",
            Harness::Grok => ".grok",
        }))
    }

    /// Kit entries linked into a profile when the kit lists none.
    pub fn default_kit_entries(self) -> &'static [&'static str] {
        match self {
            Harness::Claude => &[
                "CLAUDE.md",
                "skills",
                "commands",
                "agents",
                "rules",
                "output-styles",
                "keybindings.json",
            ],
            Harness::Codex => &["AGENTS.md", "skills", "prompts"],
            Harness::Grok => &["AGENTS.md", "skills"],
        }
    }

    /// File seeded once from the kit into a new profile, never linked.
    pub fn seed_file(self) -> &'static str {
        match self {
            Harness::Claude => "settings.json",
            Harness::Codex | Harness::Grok => "config.toml",
        }
    }
}

impl std::str::FromStr for Harness {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "claude" => Ok(Harness::Claude),
            "codex" => Ok(Harness::Codex),
            "grok" => Ok(Harness::Grok),
            other => bail!("unknown harness `{other}` (expected claude, codex or grok)"),
        }
    }
}

/// How an account authenticates against its provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Auth {
    Login,
    ApiKey {
        #[serde(default)]
        base_url: Option<String>,
        #[serde(default)]
        bearer: bool,
    },
}

/// One entry of the ledger: a harness, an identity and how to open it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub label: String,
    pub harness: Harness,
    pub auth: Auth,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub fast_model: Option<String>,
    #[serde(default)]
    pub home: Option<PathBuf>,
    #[serde(default)]
    pub native_home: bool,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "enabled")]
    pub kit: bool,
}

fn enabled() -> bool {
    true
}

impl Account {
    /// Builds a validated account with a fresh id and a managed home.
    pub fn new(label: &str, harness: Harness, auth: Auth) -> Result<Self> {
        let account = Self {
            id: uuid::Uuid::new_v4().to_string(),
            label: label.trim().to_string(),
            harness,
            auth,
            model: None,
            fast_model: None,
            home: None,
            native_home: false,
            env: BTreeMap::new(),
            args: Vec::new(),
            kit: true,
        };
        account.validate()?;
        Ok(account)
    }

    /// True when the harness CLI owns an OAuth session for this account.
    pub fn is_login(&self) -> bool {
        self.auth == Auth::Login
    }

    /// Rejects values that could break isolation or leak a key in clear text.
    pub fn validate(&self) -> Result<()> {
        uuid::Uuid::parse_str(&self.id).context("invalid account id")?;
        text("label", &self.label, 80)?;
        if let Some(model) = &self.model {
            option("model", model)?;
        }
        if let Some(model) = &self.fast_model {
            option("fast model", model)?;
        }
        if let Auth::ApiKey {
            base_url: Some(url),
            ..
        } = &self.auth
        {
            endpoint(url)?;
            if self.harness == Harness::Grok {
                bail!("Grok accounts use the xAI endpoint; a custom endpoint is not supported");
            }
        }
        for key in self.env.keys() {
            let valid = !key.is_empty()
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !key.starts_with(|c: char| c.is_ascii_digit());
            if !valid {
                bail!("invalid environment variable name `{key}`");
            }
        }
        Ok(())
    }
}

fn text(name: &str, value: &str, max: usize) -> Result<()> {
    if value.is_empty() || value.chars().count() > max || value.chars().any(char::is_control) {
        bail!("{name} must be 1 to {max} characters without control characters");
    }
    Ok(())
}

fn option(name: &str, value: &str) -> Result<()> {
    text(name, value, 200)?;
    if value.starts_with('-') || value.chars().any(char::is_whitespace) {
        bail!("{name} must not start with `-` or contain spaces");
    }
    Ok(())
}

fn endpoint(url: &str) -> Result<()> {
    text("base URL", url, 500)?;
    let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"]
        .iter()
        .any(|prefix| {
            url.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
        });
    if !(url.starts_with("https://") || local) || url.chars().any(char::is_whitespace) {
        bail!("base URL must be https:// (plain http is accepted only for localhost)");
    }
    Ok(())
}

/// Tool a provider key can be opened with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Claude,
    Codex,
    Shell,
}

impl std::str::FromStr for Tool {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "claude" => Ok(Tool::Claude),
            "codex" => Ok(Tool::Codex),
            "shell" => Ok(Tool::Shell),
            other => bail!("unknown tool `{other}` (expected claude, codex or shell)"),
        }
    }
}

/// A provider or router key that any harness or tool can be opened with.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub label: String,
    pub key_env: String,
    #[serde(default)]
    pub anthropic_url: Option<String>,
    #[serde(default)]
    pub openai_url: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub command: Vec<String>,
}

impl Provider {
    /// Builds a validated provider with a fresh id.
    pub fn new(label: &str, key_env: &str) -> Result<Self> {
        let provider = Self {
            id: uuid::Uuid::new_v4().to_string(),
            label: label.trim().to_string(),
            key_env: key_env.trim().to_string(),
            anthropic_url: None,
            openai_url: None,
            models: Vec::new(),
            command: Vec::new(),
        };
        provider.validate()?;
        Ok(provider)
    }

    /// Rejects endpoints in clear text and key variables that could hijack a process.
    pub fn validate(&self) -> Result<()> {
        uuid::Uuid::parse_str(&self.id).context("invalid provider id")?;
        text("label", &self.label, 80)?;
        let named = self.key_env.len() <= 64
            && self
                .key_env
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && self.key_env.starts_with(|c: char| c.is_ascii_uppercase())
            && ["_API_KEY", "_KEY", "_TOKEN"]
                .iter()
                .any(|suffix| self.key_env.ends_with(suffix));
        if !named {
            bail!("the key variable must look like PROVIDER_API_KEY (ending in _API_KEY, _KEY or _TOKEN)");
        }
        for url in [&self.anthropic_url, &self.openai_url]
            .into_iter()
            .flatten()
        {
            endpoint(url)?;
        }
        for model in &self.models {
            option("model", model)?;
        }
        for part in &self.command {
            text("command", part, 300)?;
        }
        Ok(())
    }

    /// Tools this provider can be opened with, most specific first.
    pub fn tools(&self) -> Vec<Tool> {
        let mut tools = Vec::new();
        if self.anthropic_url.is_some() {
            tools.push(Tool::Claude);
        }
        if self.openai_url.is_some() {
            tools.push(Tool::Codex);
        }
        tools.push(Tool::Shell);
        tools
    }
}

/// Ready-made endpoints for a well-known provider.
#[derive(Clone, Debug, Serialize)]
pub struct Preset {
    pub name: &'static str,
    pub label: &'static str,
    pub key_env: &'static str,
    pub anthropic_url: Option<&'static str>,
    pub openai_url: Option<&'static str>,
}

/// Providers Versatile Vault can fill in without the user typing endpoints.
pub fn presets() -> Vec<Preset> {
    vec![
        Preset {
            name: "openrouter",
            label: "OpenRouter",
            key_env: "OPENROUTER_API_KEY",
            anthropic_url: Some("https://openrouter.ai/api"),
            openai_url: Some("https://openrouter.ai/api/v1"),
        },
        Preset {
            name: "deepseek",
            label: "DeepSeek",
            key_env: "DEEPSEEK_API_KEY",
            anthropic_url: Some("https://api.deepseek.com/anthropic"),
            openai_url: Some("https://api.deepseek.com"),
        },
    ]
}

/// Shared instructions directory linked into every profile of one harness.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Kit {
    #[serde(default)]
    pub dir: Option<PathBuf>,
    #[serde(default)]
    pub entries: Vec<String>,
}

/// The whole ledger as persisted on disk; it never holds a secret.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub providers: Vec<Provider>,
    #[serde(default)]
    pub claude_kit: Kit,
    #[serde(default)]
    pub codex_kit: Kit,
    #[serde(default)]
    pub grok_kit: Kit,
    #[serde(default)]
    pub terminal: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            accounts: Vec::new(),
            providers: Vec::new(),
            claude_kit: Kit::default(),
            codex_kit: Kit::default(),
            grok_kit: Kit::default(),
            terminal: Vec::new(),
        }
    }
}

impl Config {
    /// Resolves an account by id, unique id prefix or exact label.
    pub fn find(&self, key: &str) -> Result<&Account> {
        let index = self.position(key)?;
        Ok(&self.accounts[index])
    }

    /// Index of the account matching an id, unique id prefix or exact label.
    pub fn position(&self, key: &str) -> Result<usize> {
        let names: Vec<(&str, &str)> = self
            .accounts
            .iter()
            .map(|a| (a.id.as_str(), a.label.as_str()))
            .collect();
        locate("account", &names, key)
    }

    /// Resolves a provider by id, unique id prefix or exact label.
    pub fn provider(&self, key: &str) -> Result<&Provider> {
        Ok(&self.providers[self.provider_position(key)?])
    }

    /// Index of the provider matching an id, unique id prefix or exact label.
    pub fn provider_position(&self, key: &str) -> Result<usize> {
        let names: Vec<(&str, &str)> = self
            .providers
            .iter()
            .map(|p| (p.id.as_str(), p.label.as_str()))
            .collect();
        locate("provider", &names, key)
    }

    /// Kit configured for a harness.
    pub fn kit(&self, harness: Harness) -> &Kit {
        match harness {
            Harness::Claude => &self.claude_kit,
            Harness::Codex => &self.codex_kit,
            Harness::Grok => &self.grok_kit,
        }
    }

    /// Mutable kit configured for a harness.
    pub fn kit_mut(&mut self, harness: Harness) -> &mut Kit {
        match harness {
            Harness::Claude => &mut self.claude_kit,
            Harness::Codex => &mut self.codex_kit,
            Harness::Grok => &mut self.grok_kit,
        }
    }
}

fn locate(kind: &str, names: &[(&str, &str)], key: &str) -> Result<usize> {
    let wanted = key.trim();
    if wanted.is_empty() {
        bail!("{kind} not found");
    }
    let mut matches: Vec<usize> = (0..names.len())
        .filter(|&i| names[i].0 == wanted || names[i].1.eq_ignore_ascii_case(wanted))
        .collect();
    if matches.is_empty() {
        matches = (0..names.len())
            .filter(|&i| names[i].0.starts_with(wanted))
            .collect();
    }
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => bail!("{kind} `{wanted}` not found"),
        _ => bail!("`{wanted}` matches more than one {kind}; use the id"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(auth: Auth) -> Result<Account> {
        Account::new("work", Harness::Claude, auth)
    }

    #[test]
    fn accepts_https_and_localhost_endpoints() {
        for url in [
            "https://openrouter.ai/api",
            "http://localhost:4000",
            "http://127.0.0.1",
        ] {
            let auth = Auth::ApiKey {
                base_url: Some(url.into()),
                bearer: true,
            };
            assert!(account(auth).is_ok(), "{url}");
        }
    }

    #[test]
    fn rejects_clear_text_remote_endpoints() {
        for url in [
            "http://example.com",
            "http://localhost.evil.com",
            "ftp://x",
            "https://a b",
        ] {
            let auth = Auth::ApiKey {
                base_url: Some(url.into()),
                bearer: false,
            };
            assert!(account(auth).is_err(), "{url}");
        }
    }

    #[test]
    fn rejects_models_that_look_like_flags() {
        let mut entry = account(Auth::Login).unwrap();
        entry.model = Some("--dangerously-skip-permissions".into());
        assert!(entry.validate().is_err());
    }

    #[test]
    fn finds_by_label_id_and_unique_prefix() {
        let mut config = Config::default();
        config.accounts.push(account(Auth::Login).unwrap());
        let id = config.accounts[0].id.clone();
        assert_eq!(config.find("WORK").unwrap().id, id);
        assert_eq!(config.find(&id).unwrap().id, id);
        assert_eq!(config.find(&id[..8]).unwrap().id, id);
        assert!(config.find("missing").is_err());
    }

    #[test]
    fn provider_key_variable_must_be_a_key_name() {
        assert!(Provider::new("DeepSeek", "DEEPSEEK_API_KEY").is_ok());
        for name in ["PATH", "LD_PRELOAD", "deepseek_api_key", "_API_KEY", "HOME"] {
            assert!(Provider::new("x", name).is_err(), "{name}");
        }
    }

    #[test]
    fn provider_offers_only_the_tools_its_endpoints_support() {
        let mut provider = Provider::new("p", "P_API_KEY").unwrap();
        assert_eq!(provider.tools(), vec![Tool::Shell]);
        provider.anthropic_url = Some("https://api.deepseek.com/anthropic".into());
        assert_eq!(provider.tools(), vec![Tool::Claude, Tool::Shell]);
        provider.openai_url = Some("http://example.com".into());
        assert!(provider.validate().is_err());
    }

    #[test]
    fn presets_are_valid_providers() {
        for preset in presets() {
            let mut provider = Provider::new(preset.label, preset.key_env).unwrap();
            provider.anthropic_url = preset.anthropic_url.map(str::to_string);
            provider.openai_url = preset.openai_url.map(str::to_string);
            assert!(provider.validate().is_ok(), "{}", preset.name);
        }
    }
}
