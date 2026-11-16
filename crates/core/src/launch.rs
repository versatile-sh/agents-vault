use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::model::{Account, Auth, Harness, Provider, Tool};
use crate::store::Store;

/// Environment variable carrying a Codex provider key into the child process.
pub const KEY_ENV: &str = "VVAULT_API_KEY";

const OPENAI_ENDPOINT: &str = "https://api.openai.com/v1";

/// Fully resolved harness invocation for one account.
#[derive(Debug)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// True for inherited variables that would override an account identity.
pub fn scrubbed(key: &str) -> bool {
    key.starts_with("ANTHROPIC_")
        || key.starts_with("CLAUDE_CODE_OAUTH_")
        || key.starts_with("CLAUDE_CODE_USE_")
        || matches!(
            key,
            "CLAUDE_CONFIG_DIR"
                | "CLAUDE_SECURESTORAGE_CONFIG_DIR"
                | "CODEX_HOME"
                | "CODEX_API_KEY"
                | "CODEX_ACCESS_TOKEN"
                | "OPENAI_API_KEY"
                | "OPENAI_BASE_URL"
                | KEY_ENV
        )
}

/// Locates the harness executable on PATH.
pub fn program(harness: Harness) -> Result<PathBuf> {
    which::which(harness.program())
        .with_context(|| format!("`{}` was not found on PATH", harness.program()))
}

/// Arguments that keep a managed profile from sharing state with other homes.
pub fn isolation_args(account: &Account) -> Vec<String> {
    if account.harness == Harness::Codex && !account.native_home {
        vec!["-c".into(), "cli_auth_credentials_store=\"file\"".into()]
    } else {
        Vec::new()
    }
}

fn home_env(store: &Store, account: &Account) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if !account.native_home {
        let home = store.home(account).to_string_lossy().into_owned();
        env.insert(account.harness.home_env().to_string(), home);
    }
    env
}

fn clean(program: &Path, env: &BTreeMap<String, String>) -> Command {
    let mut command = Command::new(program);
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(scrubbed) {
            command.env_remove(key);
        }
    }
    command.envs(env);
    command
}

/// Harness command bound to the account home, for login, status and probes.
pub fn harness_command(store: &Store, account: &Account) -> Result<Command> {
    store.ensure_home(account)?;
    let mut command = clean(&program(account.harness)?, &home_env(store, account));
    command.args(isolation_args(account));
    Ok(command)
}

fn shell_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        format!("\"{}\"", text.replace('\\', "/"))
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

fn toml_string(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

/// Resolves program, arguments and environment for an interactive session.
pub fn plan(
    store: &Store,
    account: &Account,
    secret: Option<&str>,
    cli: &Path,
    extra: &[String],
) -> Result<Launch> {
    account.validate()?;
    store.ensure_home(account)?;
    let mut env = home_env(store, account);
    let mut args = isolation_args(account);
    let key = || secret.context("this account has no API key saved; run `vvault key` to set one");
    match (account.harness, &account.auth) {
        (Harness::Claude, Auth::Login) => {
            let command = format!("{} statusline {}", shell_quote(cli), account.id);
            let settings = json!({ "statusLine": { "type": "command", "command": command } });
            args.extend(["--settings".to_string(), settings.to_string()]);
            if let Some(model) = &account.model {
                env.insert("ANTHROPIC_MODEL".into(), model.clone());
            }
        }
        (Harness::Claude, Auth::ApiKey { base_url, bearer }) => {
            if *bearer {
                env.insert("ANTHROPIC_AUTH_TOKEN".into(), key()?.to_string());
                env.insert("ANTHROPIC_API_KEY".into(), String::new());
            } else {
                env.insert("ANTHROPIC_API_KEY".into(), key()?.to_string());
            }
            if let Some(url) = base_url {
                env.insert("ANTHROPIC_BASE_URL".into(), url.clone());
            }
            if let Some(model) = &account.model {
                env.insert("ANTHROPIC_MODEL".into(), model.clone());
                if base_url.is_some() {
                    let fast = account.fast_model.as_ref().unwrap_or(model);
                    env.insert("ANTHROPIC_DEFAULT_OPUS_MODEL".into(), model.clone());
                    env.insert("ANTHROPIC_DEFAULT_SONNET_MODEL".into(), model.clone());
                    env.insert("ANTHROPIC_DEFAULT_HAIKU_MODEL".into(), fast.clone());
                }
            }
            if let (Some(fast), None) = (&account.fast_model, base_url) {
                env.insert("ANTHROPIC_DEFAULT_HAIKU_MODEL".into(), fast.clone());
            }
        }
        (Harness::Codex, Auth::Login) => {
            if let Some(model) = &account.model {
                args.extend(["-m".to_string(), model.clone()]);
            }
        }
        (Harness::Codex, Auth::ApiKey { base_url, .. }) => {
            env.insert(KEY_ENV.into(), key()?.to_string());
            let url = base_url.as_deref().unwrap_or(OPENAI_ENDPOINT);
            for (name, value) in [
                ("model_provider", "vvault".to_string()),
                ("model_providers.vvault.name", account.label.clone()),
                ("model_providers.vvault.base_url", url.to_string()),
                ("model_providers.vvault.env_key", KEY_ENV.to_string()),
                ("model_providers.vvault.wire_api", "responses".to_string()),
            ] {
                args.extend(["-c".to_string(), format!("{name}={}", toml_string(&value))]);
            }
            if let Some(model) = &account.model {
                args.extend(["-m".to_string(), model.clone()]);
            }
        }
    }
    for (name, value) in &account.env {
        if secret.is_some_and(|secret| value.contains(secret)) {
            bail!("extra environment `{name}` must not repeat the API key");
        }
        env.insert(name.clone(), value.clone());
    }
    args.extend(account.args.iter().cloned());
    args.extend(extra.iter().cloned());
    Ok(Launch {
        program: program(account.harness)?,
        args,
        env,
    })
}

/// Account view of a provider for one harness, with its own home under the provider profile.
pub fn provider_account(
    store: &Store,
    provider: &Provider,
    harness: Harness,
    model: Option<&str>,
) -> Result<Account> {
    let url = match harness {
        Harness::Claude => provider.anthropic_url.clone(),
        Harness::Codex => provider.openai_url.clone(),
    };
    let Some(url) = url else {
        bail!(
            "`{}` has no endpoint for {}",
            provider.label,
            harness.program()
        );
    };
    let mut account = Account::new(
        &provider.label,
        harness,
        Auth::ApiKey {
            base_url: Some(url),
            bearer: true,
        },
    )?;
    account.id = provider.id.clone();
    account.model = model
        .map(str::to_string)
        .or_else(|| provider.models.first().cloned());
    account.home = Some(
        store
            .root()
            .join("profiles")
            .join(&provider.id)
            .join(harness.program()),
    );
    account.validate()?;
    Ok(account)
}

fn shell() -> Result<PathBuf> {
    let name = if cfg!(windows) { "COMSPEC" } else { "SHELL" };
    let fallback = if cfg!(windows) { "cmd.exe" } else { "sh" };
    match std::env::var_os(name).filter(|value| !value.is_empty()) {
        Some(path) => Ok(PathBuf::from(path)),
        None => which::which(fallback).context("no shell was found"),
    }
}

/// Resolves a provider session: a harness on its endpoint, or a shell or tool with the key exported.
pub fn plan_provider(
    store: &Store,
    provider: &Provider,
    tool: Tool,
    model: Option<&str>,
    secret: Option<&str>,
    cli: &Path,
    extra: &[String],
) -> Result<Launch> {
    provider.validate()?;
    let harness = match tool {
        Tool::Claude => Harness::Claude,
        Tool::Codex => Harness::Codex,
        Tool::Shell => {
            let key = secret.context(
                "this provider has no API key saved; run `vvault provider key` to set one",
            )?;
            let mut env = BTreeMap::from([(provider.key_env.clone(), key.to_string())]);
            if let Some(url) = &provider.openai_url {
                env.insert("OPENAI_BASE_URL".into(), url.clone());
                env.insert("OPENAI_API_KEY".into(), key.to_string());
            }
            if let Some(url) = &provider.anthropic_url {
                env.insert("ANTHROPIC_BASE_URL".into(), url.clone());
                env.insert("ANTHROPIC_AUTH_TOKEN".into(), key.to_string());
            }
            let (program, mut args) = match provider.command.split_first() {
                Some((program, rest)) => {
                    let found = which::which(program)
                        .with_context(|| format!("`{program}` was not found on PATH"))?;
                    (found, rest.to_vec())
                }
                None => (shell()?, Vec::new()),
            };
            args.extend(extra.iter().cloned());
            return Ok(Launch { program, args, env });
        }
    };
    let account = provider_account(store, provider, harness, model)?;
    plan(store, &account, secret, cli, extra)
}

/// Replaces this process with the harness (waits for it on Windows).
pub fn exec(launch: &Launch) -> Result<i32> {
    let mut command = clean(&launch.program, &launch.env);
    command.args(&launch.args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("could not start the harness")
    }
    #[cfg(not(unix))]
    {
        let status = command.status().context("could not start the harness")?;
        Ok(status.code().unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::at(directory.path().to_path_buf()).unwrap();
        (directory, store)
    }

    fn planned(account: &Account, secret: Option<&str>) -> Result<Launch> {
        let (_guard, store) = store();
        let cli = PathBuf::from("/opt/it's here/vvault");
        plan(&store, account, secret, &cli, &["--resume".to_string()])
    }

    fn harness_available(harness: Harness) -> bool {
        program(harness).is_ok()
    }

    #[test]
    fn claude_login_gets_its_own_home_and_a_statusline_probe() {
        if !harness_available(Harness::Claude) {
            return;
        }
        let mut account = Account::new("work", Harness::Claude, Auth::Login).unwrap();
        account.model = Some("opus".into());
        let launch = planned(&account, None).unwrap();
        assert!(launch.env["CLAUDE_CONFIG_DIR"].ends_with(&account.id));
        assert_eq!(launch.env["ANTHROPIC_MODEL"], "opus");
        let settings: serde_json::Value = serde_json::from_str(&launch.args[1]).unwrap();
        let command = settings["statusLine"]["command"].as_str().unwrap();
        assert_eq!(
            command,
            format!("'/opt/it'\\''s here/vvault' statusline {}", account.id)
        );
        assert_eq!(launch.args.last().unwrap(), "--resume");
        assert!(!launch.env.contains_key("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn claude_router_key_travels_only_in_the_child_environment() {
        if !harness_available(Harness::Claude) {
            return;
        }
        let auth = Auth::ApiKey {
            base_url: Some("https://openrouter.ai/api".into()),
            bearer: true,
        };
        let mut account = Account::new("router", Harness::Claude, auth).unwrap();
        account.model = Some("vendor/big".into());
        account.fast_model = Some("vendor/small".into());
        let launch = planned(&account, Some("sk-secret")).unwrap();
        assert_eq!(launch.env["ANTHROPIC_AUTH_TOKEN"], "sk-secret");
        assert_eq!(launch.env["ANTHROPIC_API_KEY"], "");
        assert_eq!(
            launch.env["ANTHROPIC_BASE_URL"],
            "https://openrouter.ai/api"
        );
        assert_eq!(launch.env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "vendor/big");
        assert_eq!(launch.env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "vendor/small");
        assert!(launch.args.iter().all(|arg| !arg.contains("sk-secret")));
    }

    #[test]
    fn api_key_account_without_a_saved_key_does_not_launch() {
        let auth = Auth::ApiKey {
            base_url: None,
            bearer: false,
        };
        let account = Account::new("bare", Harness::Claude, auth).unwrap();
        let (_guard, store) = store();
        let error = plan(&store, &account, None, Path::new("vvault"), &[]).unwrap_err();
        assert!(error.to_string().contains("no API key"));
    }

    #[test]
    fn codex_key_account_defines_a_provider_that_reads_the_key_from_env() {
        if !harness_available(Harness::Codex) {
            return;
        }
        let auth = Auth::ApiKey {
            base_url: None,
            bearer: false,
        };
        let mut account = Account::new("my \"api\"", Harness::Codex, auth).unwrap();
        account.model = Some("gpt-5".into());
        let launch = planned(&account, Some("sk-secret")).unwrap();
        assert_eq!(launch.env[KEY_ENV], "sk-secret");
        assert!(launch.env["CODEX_HOME"].ends_with(&account.id));
        let joined = launch.args.join(" ");
        assert!(joined.contains("cli_auth_credentials_store=\"file\""));
        assert!(joined.contains("model_providers.vvault.env_key=\"VVAULT_API_KEY\""));
        assert!(joined.contains("model_providers.vvault.base_url=\"https://api.openai.com/v1\""));
        assert!(joined.contains("model_providers.vvault.name=\"my \\\"api\\\"\""));
        assert!(joined.contains("-m gpt-5"));
        assert!(!joined.contains("sk-secret"));
    }

    #[test]
    fn native_home_accounts_run_without_a_home_override() {
        if !harness_available(Harness::Codex) {
            return;
        }
        let mut account = Account::new("default", Harness::Codex, Auth::Login).unwrap();
        account.native_home = true;
        account.home = Harness::Codex.native_home();
        let launch = planned(&account, None).unwrap();
        assert!(launch.env.is_empty());
        assert_eq!(launch.args, vec!["--resume".to_string()]);
    }

    #[test]
    fn extra_environment_cannot_smuggle_the_key_into_the_ledger() {
        if !harness_available(Harness::Claude) {
            return;
        }
        let auth = Auth::ApiKey {
            base_url: None,
            bearer: false,
        };
        let mut account = Account::new("leaky", Harness::Claude, auth).unwrap();
        account.env.insert("OTHER".into(), "sk-secret".into());
        assert!(planned(&account, Some("sk-secret")).is_err());
    }

    #[test]
    fn scrubs_identity_variables_and_keeps_the_rest() {
        for key in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
            "OPENAI_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ] {
            assert!(scrubbed(key), "{key}");
        }
        for key in ["PATH", "HOME", "CLAUDE_CODE_MAX_OUTPUT_TOKENS", "TERM"] {
            assert!(!scrubbed(key), "{key}");
        }
    }

    fn provider() -> Provider {
        let mut provider = Provider::new("DeepSeek", "DEEPSEEK_API_KEY").unwrap();
        provider.anthropic_url = Some("https://api.deepseek.com/anthropic".into());
        provider.models = vec!["deepseek-chat".into(), "deepseek-reasoner".into()];
        provider
    }

    #[test]
    fn provider_opens_claude_on_its_endpoint_with_the_chosen_model() {
        if !harness_available(Harness::Claude) {
            return;
        }
        let (_guard, store) = store();
        let provider = provider();
        let launch = plan_provider(
            &store,
            &provider,
            Tool::Claude,
            Some("deepseek-reasoner"),
            Some("sk-secret"),
            Path::new("vvault"),
            &[],
        )
        .unwrap();
        assert_eq!(
            launch.env["ANTHROPIC_BASE_URL"],
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(launch.env["ANTHROPIC_AUTH_TOKEN"], "sk-secret");
        assert_eq!(launch.env["ANTHROPIC_MODEL"], "deepseek-reasoner");
        let home = store
            .root()
            .join("profiles")
            .join(&provider.id)
            .join("claude");
        assert_eq!(launch.env["CLAUDE_CONFIG_DIR"], home.to_string_lossy());
        assert!(home.is_dir());
    }

    #[test]
    fn provider_defaults_to_its_first_model() {
        let (_guard, store) = store();
        let account = provider_account(&store, &provider(), Harness::Claude, None).unwrap();
        assert_eq!(account.model.as_deref(), Some("deepseek-chat"));
    }

    #[test]
    fn provider_without_a_matching_endpoint_refuses_the_harness() {
        let (_guard, store) = store();
        let error = plan_provider(
            &store,
            &provider(),
            Tool::Codex,
            None,
            Some("k"),
            Path::new("vvault"),
            &[],
        )
        .unwrap_err();
        assert!(error.to_string().contains("no endpoint for codex"));
    }

    #[test]
    fn provider_shell_exports_the_key_under_its_own_name() {
        let (_guard, store) = store();
        let launch = plan_provider(
            &store,
            &provider(),
            Tool::Shell,
            None,
            Some("sk-secret"),
            Path::new("vvault"),
            &[],
        )
        .unwrap();
        assert_eq!(launch.env["DEEPSEEK_API_KEY"], "sk-secret");
        assert_eq!(
            launch.env["ANTHROPIC_BASE_URL"],
            "https://api.deepseek.com/anthropic"
        );
        assert!(!launch.env.contains_key("OPENAI_API_KEY"));
        assert!(launch.args.is_empty());
    }
}
