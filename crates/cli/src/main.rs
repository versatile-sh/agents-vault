use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use vvault_core::balance::Balance;
use vvault_core::ledger::{self, clearable, NewAccount, Row};
use vvault_core::limits::{self, Window};
use vvault_core::machine;
use vvault_core::model::presets;
use vvault_core::models;
use vvault_core::status::{self, Pass};
use vvault_core::store::now;
use vvault_core::{
    kit, launch, secrets, terminal, Account, Auth, Config, Harness, Provider, Store, Tool,
};

#[derive(Parser)]
#[command(
    name = "vvault",
    version,
    about = "One ledger for every Claude Code and Codex account, key and router"
)]
struct Cli {
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// List accounts with their session state and limits
    List {
        #[arg(long)]
        json: bool,
        #[arg(long, help = "Ask Codex for fresh limits instead of using the cache")]
        refresh: bool,
    },
    /// Add an account; without --api-key it signs in through the harness itself
    Add {
        label: String,
        #[arg(long, default_value = "claude")]
        harness: Harness,
        #[arg(long, help = "Authenticate with an API key kept in the OS keychain")]
        api_key: bool,
        #[arg(long, help = "Provider or router endpoint (implies --api-key)")]
        base_url: Option<String>,
        #[arg(long, help = "Send the key as a bearer token (routers and gateways)")]
        bearer: bool,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        fast_model: Option<String>,
        #[arg(long, help = "Read the key from stdin instead of prompting")]
        key_stdin: bool,
        #[arg(long, help = "Create the account without signing in now")]
        no_login: bool,
    },
    /// Track a harness home that already exists, in place and without copying it
    Adopt {
        label: String,
        #[arg(long, default_value = "claude")]
        harness: Harness,
        #[arg(
            long,
            help = "Existing home; defaults to the harness default (~/.claude, ~/.codex)"
        )]
        dir: Option<PathBuf>,
    },
    /// Sign an account in again through the harness
    Login {
        account: String,
        #[arg(long, hide = true)]
        hold: bool,
    },
    /// Start the harness for an account in this terminal
    Run {
        account: String,
        #[arg(long, hide = true)]
        hold: bool,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Open a new terminal window running the account
    Open { account: String },
    /// Manage provider and router keys (DeepSeek, OpenRouter, any compatible endpoint)
    Provider {
        #[command(subcommand)]
        command: ProviderAction,
    },
    /// Start a tool on a provider key: claude, codex, or a shell with the key exported
    Use {
        provider: String,
        #[arg(
            long = "with",
            help = "claude, codex or shell; defaults to the first the provider supports"
        )]
        tool: Option<Tool>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, help = "Open a new terminal window instead of using this one")]
        window: bool,
        #[arg(long, hide = true)]
        hold: bool,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Change label, models or kit usage of an account
    Set {
        account: String,
        #[arg(long)]
        label: Option<String>,
        #[arg(long, help = "Model to start with; pass an empty value to clear")]
        model: Option<String>,
        #[arg(long)]
        fast_model: Option<String>,
        #[arg(long)]
        kit: Option<bool>,
    },
    /// Save or replace the API key of an account in the OS keychain
    Key {
        account: String,
        #[arg(long)]
        key_stdin: bool,
    },
    /// Remove an account from the ledger; its home is kept unless --purge
    Remove {
        account: String,
        #[arg(long, help = "Also delete the managed profile directory")]
        purge: bool,
    },
    /// Set the shared instructions folder linked into every profile of a harness
    Kit {
        harness: Harness,
        #[arg(
            long,
            help = "Folder with CLAUDE.md / AGENTS.md, skills, commands…; omit to show"
        )]
        dir: Option<PathBuf>,
        #[arg(long, help = "Stop linking a kit for this harness")]
        clear: bool,
    },
    /// Choose the terminal command used by `open` (empty restores auto-detection)
    Terminal { command: Vec<String> },
    /// List the models an account, a harness or a provider key can use
    Models {
        #[arg(
            help = "Account or provider; omit with --harness to see what a new account would offer"
        )]
        name: Option<String>,
        #[arg(long)]
        harness: Option<Harness>,
    },
    /// Make an account the one plain `claude`, `codex` or `grok` uses in new terminals and apps
    Default {
        account: Option<String>,
        #[arg(long, value_name = "HARNESS", help = "Stop overriding this harness")]
        clear: Option<Harness>,
    },
    /// Print the shell code that applies the machine defaults (used by the shell hook)
    Env {
        #[arg(
            long,
            help = "bash, zsh, fish or powershell; detected from $SHELL when omitted"
        )]
        shell: Option<String>,
    },
    /// Renew every signed-in account that is due
    Keepalive {
        #[arg(long)]
        force: bool,
    },
    /// Print a Waybar-compatible JSON block with the busiest account
    Waybar,
    #[command(hide = true)]
    Statusline { account: String },
}

#[derive(Subcommand)]
enum ProviderAction {
    /// Add a provider key; --preset fills the endpoints for known providers
    Add {
        label: String,
        #[arg(long, help = "openrouter or deepseek")]
        preset: Option<String>,
        #[arg(long, help = "Anthropic-compatible endpoint, used by Claude Code")]
        anthropic_url: Option<String>,
        #[arg(
            long,
            help = "OpenAI-compatible endpoint, used by Codex and most other tools"
        )]
        openai_url: Option<String>,
        #[arg(long, help = "Variable the key is exported as, e.g. DEEPSEEK_API_KEY")]
        key_env: Option<String>,
        #[arg(
            long = "model",
            help = "Model offered for this provider; repeat for more"
        )]
        models: Vec<String>,
        #[arg(long, num_args = 1.., help = "Tool started by `--with shell` instead of your shell, e.g. opencode")]
        command: Vec<String>,
        #[arg(long)]
        key_stdin: bool,
    },
    /// List providers
    List {
        #[arg(long)]
        json: bool,
    },
    /// Replace the models offered for a provider
    Models {
        provider: String,
        models: Vec<String>,
    },
    /// Save or replace the key of a provider in the OS keychain
    Key {
        provider: String,
        #[arg(long)]
        key_stdin: bool,
    },
    /// Remove a provider and its key
    Remove {
        provider: String,
        #[arg(long, help = "Also delete the profiles created for it")]
        purge: bool,
    },
}

fn read_key(from_stdin: bool) -> Result<String> {
    let key = if from_stdin || !std::io::stdin().is_terminal() {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        text
    } else {
        rpassword::prompt_password("API key (hidden): ")?
    };
    let key = key.trim().to_string();
    if key.is_empty() {
        bail!("no API key was given");
    }
    Ok(key)
}

fn percent(window: &Option<Window>) -> String {
    window
        .as_ref()
        .map_or("—".into(), |w| format!("{:.0}%", w.percent))
}

fn reset(window: &Option<Window>) -> String {
    let Some(at) = window.as_ref().and_then(|w| w.resets_at) else {
        return String::new();
    };
    let minutes = (at - now()).max(0) / 60;
    match minutes {
        m if m >= 2880 => format!("{}d", m / 1440),
        m if m >= 60 => format!("{}h{:02}", m / 60, m % 60),
        m => format!("{m}m"),
    }
}

fn money(balance: &Option<Balance>) -> Option<String> {
    let balance = balance.as_ref()?;
    match (balance.remaining, balance.used) {
        (Some(left), _) => Some(format!("{left:.2} {} left", balance.currency)),
        (None, Some(used)) => Some(format!("{used:.2} {} used", balance.currency)),
        _ => None,
    }
}

fn describe(account: &Account) -> &'static str {
    match (&account.auth, account.harness) {
        (Auth::Login, _) => "login",
        (
            Auth::ApiKey {
                base_url: Some(_), ..
            },
            _,
        ) => "router",
        (Auth::ApiKey { .. }, _) => "api key",
    }
}

fn list(store: &Store, as_json: bool, refresh: bool) -> Result<()> {
    let config = store.load()?;
    let rows = ledger::rows(store, &config, if refresh { 0 } else { 60 });
    if as_json {
        let accounts: Vec<Value> = rows
            .iter()
            .map(
                |Row {
                     account,
                     limits,
                     status,
                     balance,
                     default,
                 }| {
                    json!({
                        "id": account.id, "label": account.label, "harness": account.harness,
                        "auth": describe(account), "model": account.model,
                        "limits": limits, "status": status, "balance": balance, "default": default,
                    })
                },
            )
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "accounts": accounts }))?
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("No accounts yet. Try `vvault add work` or `vvault adopt personal`.");
        return Ok(());
    }
    println!(
        "{:<20} {:<7} {:<8} {:>5} {:>6}  {:>5} {:>6}  STATE",
        "ACCOUNT", "HARNESS", "AUTH", "5H", "RESET", "7D", "RESET"
    );
    for Row {
        account,
        limits,
        status,
        balance,
        ..
    } in rows
    {
        let limits = limits.unwrap_or_default();
        let state = match (&account.auth, &status) {
            (Auth::Login, Some(s)) if !s.logged_in => "needs login".to_string(),
            (Auth::Login, Some(s)) => s.email.clone().unwrap_or_else(|| "signed in".into()),
            (Auth::Login, None) => "not checked".into(),
            _ => [money(&balance), account.model.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("  "),
        };
        println!(
            "{:<20} {:<7} {:<8} {:>5} {:>6}  {:>5} {:>6}  {}",
            account.label.chars().take(20).collect::<String>(),
            account.harness.program(),
            describe(&account),
            percent(&limits.session),
            reset(&limits.session),
            percent(&limits.weekly),
            reset(&limits.weekly),
            state
        );
    }
    Ok(())
}

fn prepare(store: &Store, config: &Config, account: &Account) -> Result<()> {
    for warning in kit::apply(store, config, account)? {
        eprintln!("vvault: {warning}");
    }
    Ok(())
}

fn login(store: &Store, key: &str) -> Result<()> {
    let config = store.load()?;
    let account = config.find(key)?;
    if !account.is_login() {
        bail!(
            "`{}` uses an API key; update it with `vvault key`",
            account.label
        );
    }
    prepare(store, &config, account)?;
    let mut command = launch::harness_command(store, account)?;
    match account.harness {
        Harness::Claude => command.args(["auth", "login"]),
        Harness::Codex | Harness::Grok => command.arg("login"),
    };
    let finished = command
        .status()
        .context("could not start the harness login")?;
    let state = status::check(store, account)?;
    if !finished.success() || !state.logged_in {
        bail!("sign-in was not completed for `{}`", account.label);
    }
    println!(
        "{} is signed in{}.",
        account.label,
        state.email.map(|e| format!(" as {e}")).unwrap_or_default()
    );
    Ok(())
}

fn run(store: &Store, key: &str, extra: &[String]) -> Result<i32> {
    let config = store.load()?;
    let account = config.find(key)?;
    prepare(store, &config, account)?;
    let secret = if account.is_login() {
        None
    } else {
        secrets::get(&account.id)?
    };
    let plan = launch::plan(
        store,
        account,
        secret.as_deref(),
        &terminal::cli_path()?,
        extra,
    )?;
    launch::exec(&plan)
}

fn provider(store: &Store, action: ProviderAction) -> Result<()> {
    match action {
        ProviderAction::Add {
            label,
            preset,
            anthropic_url,
            openai_url,
            key_env,
            models,
            command,
            key_stdin,
        } => {
            let preset = match preset {
                Some(name) => Some(
                    presets()
                        .into_iter()
                        .find(|preset| preset.name.eq_ignore_ascii_case(&name))
                        .with_context(|| format!("unknown preset `{name}`"))?,
                ),
                None => None,
            };
            let key_env = key_env
                .or(preset.as_ref().map(|preset| preset.key_env.to_string()))
                .context("pass --key-env (for example MYPROVIDER_API_KEY) or a --preset")?;
            let mut entry = Provider::new(&label, &key_env)?;
            let from = |given: Option<String>, known: Option<&str>| {
                given.and_then(clearable).or(known.map(str::to_string))
            };
            entry.anthropic_url = from(
                anthropic_url,
                preset.as_ref().and_then(|preset| preset.anthropic_url),
            );
            entry.openai_url = from(
                openai_url,
                preset.as_ref().and_then(|preset| preset.openai_url),
            );
            entry.models = models;
            entry.command = command;
            entry.validate()?;
            let saved = ledger::create_provider(store, entry, &read_key(key_stdin)?)?;
            println!(
                "Added {}. Start it with `vvault use {}`.",
                saved.label, saved.label
            );
        }
        ProviderAction::List { json } => {
            let config = store.load()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({ "providers": config.providers }))?
                );
                return Ok(());
            }
            if config.providers.is_empty() {
                println!(
                    "No providers yet. Try `vvault provider add OpenRouter --preset openrouter`."
                );
            }
            for entry in &config.providers {
                let tools: Vec<&str> = entry
                    .tools()
                    .iter()
                    .map(|tool| match tool {
                        Tool::Claude => "claude",
                        Tool::Codex => "codex",
                        Tool::Shell => "shell",
                    })
                    .collect();
                println!(
                    "{:<20} {:<22} {:<20} {}",
                    entry.label,
                    entry.key_env,
                    tools.join(","),
                    entry.models.join(" ")
                );
            }
        }
        ProviderAction::Models { provider, models } => {
            store.update(|config| {
                let index = config.provider_position(&provider)?;
                config.providers[index].models = models.clone();
                Ok(())
            })?;
        }
        ProviderAction::Key {
            provider,
            key_stdin,
        } => {
            let config = store.load()?;
            let entry = config.provider(&provider)?;
            secrets::set(&entry.id, &read_key(key_stdin)?)?;
            println!("Key saved in the OS keychain for {}.", entry.label);
        }
        ProviderAction::Remove { provider, purge } => {
            let (removed, purged) = ledger::remove_provider(store, &provider, purge)?;
            println!(
                "Removed {}{}.",
                removed.label,
                if purged { " and its profiles" } else { "" }
            );
        }
    }
    Ok(())
}

fn use_provider(
    store: &Store,
    key: &str,
    tool: Option<Tool>,
    model: Option<String>,
    window: bool,
    extra: &[String],
) -> Result<i32> {
    let config = store.load()?;
    let entry = config.provider(key)?;
    let tool = tool.unwrap_or(entry.tools()[0]);
    let model = model.and_then(clearable);
    if window {
        terminal::open_provider(
            entry,
            tool,
            model.as_deref(),
            &config.terminal,
            std::env::current_dir().ok().as_deref(),
        )?;
        return Ok(0);
    }
    let harness = match tool {
        Tool::Claude => Some(Harness::Claude),
        Tool::Codex => Some(Harness::Codex),
        Tool::Shell => None,
    };
    if let Some(harness) = harness {
        let account = launch::provider_account(store, entry, harness, model.as_deref())?;
        prepare(store, &config, &account)?;
    }
    let secret = secrets::get(&entry.id)?;
    let plan = launch::plan_provider(
        store,
        entry,
        tool,
        model.as_deref(),
        secret.as_deref(),
        &terminal::cli_path()?,
        extra,
    )?;
    launch::exec(&plan)
}

fn statusline(store: &Store, key: &str) -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let config = store.load()?;
    let account = config.find(key)?;
    let parsed: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let captured = limits::capture_claude(store, &account.id, &parsed)
        .ok()
        .flatten();
    let settings: Option<Value> =
        std::fs::read_to_string(store.home(account).join("settings.json"))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
    let chained = settings
        .as_ref()
        .and_then(|value| value.pointer("/statusLine/command"))
        .and_then(Value::as_str)
        .filter(|command| !command.contains(" statusline "));
    if let Some(command) = chained {
        let shell = if which_shell() {
            ("sh", "-c")
        } else {
            ("cmd", "/C")
        };
        let mut child = Command::new(shell.0)
            .arg(shell.1)
            .arg(command)
            .stdin(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.as_bytes());
        }
        child.wait()?;
        return Ok(());
    }
    let limits = captured
        .or_else(|| limits::read(store, &account.id))
        .unwrap_or_default();
    println!(
        "{} · 5h {} · 7d {}",
        account.label,
        percent(&limits.session),
        percent(&limits.weekly)
    );
    Ok(())
}

fn which_shell() -> bool {
    cfg!(unix)
        || Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .status()
            .is_ok_and(|status| status.success())
}

fn waybar(store: &Store) -> Result<()> {
    let config = store.load()?;
    let rows = ledger::rows(store, &config, 60);
    let mut worst = 0.0_f64;
    let mut tooltip = Vec::new();
    for Row {
        account, limits, ..
    } in &rows
    {
        let Some(limits) = limits else { continue };
        worst = worst.max(limits.session.as_ref().map_or(0.0, |w| w.percent));
        tooltip.push(format!(
            "{}  5h {}  7d {}",
            account.label,
            percent(&limits.session),
            percent(&limits.weekly)
        ));
    }
    let class = match worst {
        p if p >= 90.0 => "critical",
        p if p >= 70.0 => "warning",
        _ => "ok",
    };
    println!(
        "{}",
        json!({ "text": format!("{worst:.0}%"), "tooltip": tooltip.join("\n"), "class": class })
    );
    Ok(())
}

fn dispatch(store: &Store, action: Action) -> Result<i32> {
    match action {
        Action::List { json, refresh } => list(store, json, refresh)?,
        Action::Add {
            label,
            harness,
            api_key,
            base_url,
            bearer,
            model,
            fast_model,
            key_stdin,
            no_login,
        } => {
            if api_key || base_url.is_some() {
                let auth = Auth::ApiKey {
                    base_url: base_url.and_then(clearable),
                    bearer,
                };
                Account::new(&label, harness, auth.clone())?;
                let key = Some(read_key(key_stdin)?);
                let account = ledger::create(
                    store,
                    NewAccount {
                        label,
                        harness,
                        auth,
                        model,
                        fast_model,
                        key,
                    },
                )?;
                println!(
                    "Added {} ({}). Open it with `vvault open {}`.",
                    account.label,
                    describe(&account),
                    account.label
                );
            } else {
                let account = ledger::create(
                    store,
                    NewAccount {
                        label,
                        harness,
                        auth: Auth::Login,
                        model,
                        fast_model,
                        key: None,
                    },
                )?;
                if no_login {
                    println!(
                        "Added {}. Sign in with `vvault login {}`.",
                        account.label, account.label
                    );
                } else {
                    login(store, &account.id)?;
                }
            }
        }
        Action::Adopt {
            label,
            harness,
            dir,
        } => {
            let account = ledger::adopt(store, &label, harness, dir)?;
            println!(
                "Tracking {} at {} (nothing was copied).",
                account.label,
                store.home(&account).display()
            );
        }
        Action::Login { account, .. } => login(store, &account)?,
        Action::Run { account, args, .. } => return run(store, &account, &args),
        Action::Open { account } => {
            let config = store.load()?;
            let account = config.find(&account)?;
            terminal::open_account(
                account,
                "run",
                &config.terminal,
                std::env::current_dir().ok().as_deref(),
            )?;
        }
        Action::Provider { command } => provider(store, command)?,
        Action::Use {
            provider,
            tool,
            model,
            window,
            args,
            ..
        } => return use_provider(store, &provider, tool, model, window, &args),
        Action::Set {
            account,
            label,
            model,
            fast_model,
            kit,
        } => {
            store.update(|config| {
                let index = config.position(&account)?;
                let entry = &mut config.accounts[index];
                if let Some(label) = label {
                    entry.label = label.trim().to_string();
                }
                if let Some(model) = model {
                    entry.model = clearable(model);
                }
                if let Some(model) = fast_model {
                    entry.fast_model = clearable(model);
                }
                if let Some(kit) = kit {
                    entry.kit = kit;
                }
                Ok(())
            })?;
        }
        Action::Key { account, key_stdin } => {
            let config = store.load()?;
            let account = config.find(&account)?;
            if account.is_login() {
                bail!(
                    "`{}` signs in through the harness and has no API key",
                    account.label
                );
            }
            secrets::set(&account.id, &read_key(key_stdin)?)?;
            println!("Key saved in the OS keychain for {}.", account.label);
        }
        Action::Remove { account, purge } => {
            let (removed, purged) = ledger::remove(store, &account, purge)?;
            if purged {
                println!("Removed {} and its profile.", removed.label);
            } else {
                println!(
                    "Removed {} from the ledger; {} was left untouched.",
                    removed.label,
                    store.home(&removed).display()
                );
            }
        }
        Action::Kit {
            harness,
            dir,
            clear,
        } => {
            if clear || dir.is_some() {
                let dir = match dir {
                    Some(path) => Some(
                        path.canonicalize()
                            .with_context(|| format!("{} does not exist", path.display()))?,
                    ),
                    None => None,
                };
                store.update(|config| {
                    config.kit_mut(harness).dir = dir.clone();
                    Ok(())
                })?;
            }
            let config = store.load()?;
            match &config.kit(harness).dir {
                Some(path) => println!("{} kit: {}", harness.program(), path.display()),
                None => println!("{} kit: not set", harness.program()),
            }
        }
        Action::Terminal { command } => {
            store.update(|config| {
                config.terminal = command.clone();
                Ok(())
            })?;
        }
        Action::Keepalive { force } => {
            let config = store.load()?;
            let mut failed = false;
            for (account, pass) in status::keepalive(store, &config, force) {
                let line = match pass {
                    Pass::Checked(state) if state.logged_in => "session alive".to_string(),
                    Pass::Checked(_) => {
                        failed = true;
                        "needs login".to_string()
                    }
                    Pass::Fresh => "checked recently".to_string(),
                    Pass::Busy => "another keep-alive is running".to_string(),
                    Pass::Failed(error) => {
                        failed = true;
                        error
                    }
                };
                println!("{}: {line}", account.label);
            }
            return Ok(i32::from(failed));
        }
        Action::Models { name, harness } => {
            let config = store.load()?;
            let models = match (name, harness) {
                (Some(name), _) => match config.find(&name) {
                    Ok(account) => models::for_account(store, account)?,
                    Err(_) => {
                        let provider = config.provider(&name)?;
                        let key = secrets::get(&provider.id)?
                            .context("no API key saved for this provider")?;
                        models::for_provider(provider, &key)?
                    }
                },
                (None, Some(harness)) => models::for_harness(store, harness)?,
                (None, None) => bail!("name an account or provider, or pass --harness"),
            };
            for model in models {
                println!("{:<34} {}", model.id, model.label);
            }
        }
        Action::Default { account, clear } => match (account, clear) {
            (Some(account), _) => {
                let chosen = machine::set_default(store, &account)?;
                println!(
                    "{} is now the default {} account. New terminals and apps pick it up; run `eval \"$(vvault env)\"` to apply it here.",
                    chosen.label,
                    chosen.harness.program()
                );
            }
            (None, Some(harness)) => {
                machine::clear_default(store, harness)?;
                println!(
                    "{} goes back to its own default home in new terminals.",
                    harness.program()
                );
            }
            (None, None) => {
                let config = store.load()?;
                for (harness, id) in &config.defaults {
                    let label = config
                        .find(id)
                        .map(|account| account.label.clone())
                        .unwrap_or_else(|_| "missing".into());
                    println!("{harness}: {label}");
                }
            }
        },
        Action::Env { shell } => {
            let shell = shell.unwrap_or_else(|| {
                let path = std::env::var("SHELL").unwrap_or_default();
                path.rsplit('/').next().unwrap_or("bash").to_string()
            });
            print!(
                "{}",
                machine::snippet(&shell, &machine::exports(store, &store.load()?))
            );
        }
        Action::Waybar => waybar(store)?,
        Action::Statusline { account } => statusline(store, &account)?,
    }
    Ok(0)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let hold = matches!(
        &cli.command,
        Action::Run { hold: true, .. } | Action::Login { hold: true, .. }
    );
    let quiet = matches!(&cli.command, Action::Statusline { .. });
    let outcome = Store::open().and_then(|store| dispatch(&store, cli.command));
    let code = match outcome {
        Ok(code) => code,
        Err(_) if quiet => 0,
        Err(error) => {
            eprintln!("vvault: {error:#}");
            1
        }
    };
    if hold && std::io::stdin().is_terminal() && (code != 0 || cfg!(not(unix))) {
        eprint!("\nPress Enter to close.");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    ExitCode::from(code.clamp(0, 255) as u8)
}
