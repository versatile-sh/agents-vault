#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use vvault_core::ledger::{self, clearable, NewAccount, ProviderRow, Row};
use vvault_core::limits::Window;
use vvault_core::login::{self, Session};
use vvault_core::model::{presets, Preset};
use vvault_core::models::{self, ModelOption};
use vvault_core::{limits, secrets, status, terminal, Auth, Harness, Kit, Provider, Store, Tool};

const TRAY_ID: &str = "vvault";
const REFRESH: Duration = Duration::from_secs(60);
const KEEPALIVE_EVERY: u32 = 10;

const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);

type Reply<T> = Result<T, String>;

#[derive(Clone, Default)]
struct Logins(Arc<Mutex<HashMap<String, Session>>>);

#[derive(Clone, Serialize)]
struct LoginResult {
    id: String,
    label: String,
    ok: bool,
}

fn fail(error: anyhow::Error) -> String {
    format!("{error:#}")
}

#[derive(Deserialize)]
struct AccountForm {
    label: String,
    harness: Harness,
    api_key: bool,
    base_url: Option<String>,
    bearer: bool,
    model: Option<String>,
    fast_model: Option<String>,
    key: Option<String>,
}

#[derive(Deserialize)]
struct AccountPatch {
    label: String,
    model: Option<String>,
    fast_model: Option<String>,
    kit: bool,
}

#[derive(Serialize, Deserialize)]
struct Settings {
    claude_kit: Option<PathBuf>,
    codex_kit: Option<PathBuf>,
    grok_kit: Option<PathBuf>,
    terminal: Vec<String>,
}

fn snapshot(store: &Store, max_age: i64) -> Reply<Vec<Row>> {
    let config = store.load().map_err(fail)?;
    Ok(ledger::rows(store, &config, max_age))
}

#[tauri::command(async)]
fn accounts(store: State<Store>, refresh: bool) -> Reply<Vec<Row>> {
    snapshot(&store, if refresh { 0 } else { 60 })
}

fn open_in_terminal(store: &Store, id: &str, action: &str) -> Reply<()> {
    let config = store.load().map_err(fail)?;
    let account = config.find(id).map_err(fail)?;
    let home = directories_home();
    terminal::open_account(account, action, &config.terminal, home.as_deref()).map_err(fail)
}

fn directories_home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

#[tauri::command(async)]
fn open_account(store: State<Store>, id: String) -> Reply<()> {
    open_in_terminal(&store, &id, "run")
}

fn watch_login(app: AppHandle, logins: Logins, id: String, label: String) {
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let finished = loop {
            std::thread::sleep(Duration::from_millis(400));
            let Ok(mut sessions) = logins.0.lock() else {
                return;
            };
            let Some(session) = sessions.get_mut(&id) else {
                return;
            };
            let done = session
                .finished()
                .or((started.elapsed() > LOGIN_TIMEOUT).then_some(false));
            if let Some(done) = done {
                sessions.remove(&id);
                break done;
            }
        };
        let store = app.state::<Store>();
        let account = store
            .load()
            .ok()
            .and_then(|config| config.find(&id).ok().cloned());
        let signed_in = account.as_ref().is_some_and(|account| {
            let state = status::check(&store, account).is_ok_and(|state| state.logged_in);
            if state {
                limits::current(&store, account, 0);
            }
            state
        });
        refresh_tray(&app);
        let _ = app.emit(
            "login-finished",
            LoginResult {
                id,
                label,
                ok: finished && signed_in,
            },
        );
    });
}

fn start_login(app: &AppHandle, store: &Store, logins: &Logins, id: &str) -> Reply<()> {
    let config = store.load().map_err(fail)?;
    let account = config.find(id).map_err(fail)?;
    let session = login::start(store, &config, account).map_err(fail)?;
    logins
        .0
        .lock()
        .map_err(|_| "sign-in state is unavailable")?
        .insert(account.id.clone(), session);
    watch_login(
        app.clone(),
        logins.clone(),
        account.id.clone(),
        account.label.clone(),
    );
    Ok(())
}

#[tauri::command(async)]
fn login_account(
    app: AppHandle,
    store: State<Store>,
    logins: State<Logins>,
    id: String,
) -> Reply<()> {
    start_login(&app, &store, &logins, &id)
}

#[tauri::command(async)]
fn login_code(logins: State<Logins>, id: String, code: String) -> Reply<()> {
    let mut sessions = logins
        .0
        .lock()
        .map_err(|_| "sign-in state is unavailable")?;
    let session = sessions.get_mut(&id).ok_or("this sign-in already ended")?;
    session.send_code(&code).map_err(fail)
}

#[tauri::command(async)]
fn login_reopen(logins: State<Logins>, id: String) -> Reply<()> {
    let url = {
        let sessions = logins
            .0
            .lock()
            .map_err(|_| "sign-in state is unavailable")?;
        sessions.get(&id).and_then(Session::url)
    };
    login::open_browser(&url.ok_or("the sign-in page is not ready yet")?).map_err(fail)
}

#[tauri::command(async)]
fn login_cancel(logins: State<Logins>, id: String) -> Reply<()> {
    if let Ok(mut sessions) = logins.0.lock() {
        sessions.remove(&id);
    }
    Ok(())
}

#[tauri::command(async)]
fn account_models(store: State<Store>, id: String) -> Reply<Vec<ModelOption>> {
    let config = store.load().map_err(fail)?;
    models::for_account(&store, config.find(&id).map_err(fail)?).map_err(fail)
}

#[tauri::command(async)]
fn harness_models(store: State<Store>, harness: Harness) -> Reply<Vec<ModelOption>> {
    models::for_harness(&store, harness).map_err(fail)
}

#[tauri::command(async)]
fn key_models(
    harness: Harness,
    base_url: Option<String>,
    bearer: bool,
    key: String,
) -> Reply<Vec<ModelOption>> {
    let base_url = base_url.and_then(clearable);
    let probe = Auth::ApiKey {
        base_url: base_url.clone(),
        bearer,
    };
    vvault_core::Account::new("probe", harness, probe).map_err(fail)?;
    models::remote(harness, base_url.as_deref(), bearer, key.trim()).map_err(fail)
}

#[tauri::command(async)]
fn provider_models(
    store: State<Store>,
    id: Option<String>,
    form: ProviderForm,
) -> Reply<Vec<ModelOption>> {
    let mut provider = Provider::new("probe", "PROBE_API_KEY").map_err(fail)?;
    let typed = form.apply(&mut provider);
    provider.validate().map_err(fail)?;
    let saved = match (&typed, id) {
        (None, Some(id)) => {
            let config = store.load().map_err(fail)?;
            secrets::get(&config.provider(&id).map_err(fail)?.id).map_err(fail)?
        }
        _ => None,
    };
    let key = typed.or(saved).ok_or("type the API key first")?;
    models::for_provider(&provider, &key).map_err(fail)
}

#[tauri::command(async)]
fn add_account(
    app: AppHandle,
    store: State<Store>,
    logins: State<Logins>,
    form: AccountForm,
) -> Reply<String> {
    let auth = if form.api_key {
        Auth::ApiKey {
            base_url: form.base_url.and_then(clearable),
            bearer: form.bearer,
        }
    } else {
        Auth::Login
    };
    let new = NewAccount {
        label: form.label,
        harness: form.harness,
        auth,
        model: form.model,
        fast_model: form.fast_model,
        key: form.key.filter(|_| form.api_key),
    };
    let account = ledger::create(&store, new).map_err(fail)?;
    refresh_tray(&app);
    if account.is_login() {
        start_login(&app, &store, &logins, &account.id)?;
    }
    Ok(account.id)
}

#[tauri::command(async)]
fn adopt_account(
    app: AppHandle,
    store: State<Store>,
    label: String,
    harness: Harness,
    dir: Option<String>,
) -> Reply<()> {
    let dir = dir.and_then(clearable).map(PathBuf::from);
    ledger::adopt(&store, &label, harness, dir).map_err(fail)?;
    refresh_tray(&app);
    Ok(())
}

#[tauri::command(async)]
fn update_account(
    app: AppHandle,
    store: State<Store>,
    id: String,
    patch: AccountPatch,
) -> Reply<()> {
    store
        .update(|config| {
            let index = config.position(&id)?;
            let entry = &mut config.accounts[index];
            entry.label = patch.label.trim().to_string();
            entry.model = patch.model.clone().and_then(clearable);
            entry.fast_model = patch.fast_model.clone().and_then(clearable);
            entry.kit = patch.kit;
            Ok(())
        })
        .map_err(fail)?;
    refresh_tray(&app);
    Ok(())
}

#[tauri::command(async)]
fn set_key(store: State<Store>, id: String, key: String) -> Reply<()> {
    let config = store.load().map_err(fail)?;
    let account = config.find(&id).map_err(fail)?;
    if account.is_login() {
        return Err("this account signs in through the harness and has no API key".into());
    }
    secrets::set(&account.id, &key).map_err(fail)
}

#[tauri::command(async)]
fn remove_account(app: AppHandle, store: State<Store>, id: String, purge: bool) -> Reply<()> {
    ledger::remove(&store, &id, purge).map_err(fail)?;
    refresh_tray(&app);
    Ok(())
}

#[tauri::command(async)]
fn settings(store: State<Store>) -> Reply<Settings> {
    let config = store.load().map_err(fail)?;
    Ok(Settings {
        claude_kit: config.claude_kit.dir,
        codex_kit: config.codex_kit.dir,
        grok_kit: config.grok_kit.dir,
        terminal: config.terminal,
    })
}

fn kit_dir(path: Option<PathBuf>) -> anyhow::Result<Option<PathBuf>> {
    let Some(path) = path.filter(|path| !path.as_os_str().is_empty()) else {
        return Ok(None);
    };
    let resolved = path
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("{} does not exist", path.display()))?;
    if !resolved.is_dir() {
        anyhow::bail!("{} is not a folder", resolved.display());
    }
    Ok(Some(resolved))
}

#[tauri::command(async)]
fn save_settings(store: State<Store>, settings: Settings) -> Reply<()> {
    let claude = kit_dir(settings.claude_kit).map_err(fail)?;
    let codex = kit_dir(settings.codex_kit).map_err(fail)?;
    let grok = kit_dir(settings.grok_kit).map_err(fail)?;
    store
        .update(|config| {
            config.claude_kit = Kit {
                dir: claude.clone(),
                entries: config.claude_kit.entries.clone(),
            };
            config.codex_kit = Kit {
                dir: codex.clone(),
                entries: config.codex_kit.entries.clone(),
            };
            config.grok_kit.dir = grok.clone();
            config.terminal = settings
                .terminal
                .iter()
                .filter(|part| !part.trim().is_empty())
                .cloned()
                .collect();
            Ok(())
        })
        .map_err(fail)
}

#[derive(Deserialize)]
struct ProviderForm {
    label: String,
    key_env: String,
    anthropic_url: Option<String>,
    openai_url: Option<String>,
    models: Vec<String>,
    command: Vec<String>,
    key: Option<String>,
}

impl ProviderForm {
    fn apply(self, provider: &mut Provider) -> Option<String> {
        let words =
            |items: Vec<String>| items.into_iter().filter_map(clearable).collect::<Vec<_>>();
        provider.label = self.label.trim().to_string();
        provider.key_env = self.key_env.trim().to_string();
        provider.anthropic_url = self.anthropic_url.and_then(clearable);
        provider.openai_url = self.openai_url.and_then(clearable);
        provider.models = words(self.models);
        provider.command = words(self.command);
        self.key.and_then(clearable)
    }
}

#[tauri::command(async)]
fn providers(store: State<Store>, refresh: bool) -> Reply<Vec<ProviderRow>> {
    let config = store.load().map_err(fail)?;
    Ok(ledger::provider_rows(
        &store,
        &config,
        if refresh { 0 } else { 120 },
    ))
}

#[tauri::command]
fn provider_presets() -> Vec<Preset> {
    presets()
}

#[tauri::command(async)]
fn add_provider(app: AppHandle, store: State<Store>, form: ProviderForm) -> Reply<()> {
    let mut provider = Provider::new(&form.label, &form.key_env).map_err(fail)?;
    let key = form.apply(&mut provider).ok_or("an API key is required")?;
    ledger::create_provider(&store, provider, &key).map_err(fail)?;
    refresh_tray(&app);
    Ok(())
}

#[tauri::command(async)]
fn update_provider(
    app: AppHandle,
    store: State<Store>,
    id: String,
    form: ProviderForm,
) -> Reply<()> {
    let mut slot = Some(form);
    let (provider_id, key) = store
        .update(|config| {
            let index = config.provider_position(&id)?;
            let form = slot
                .take()
                .ok_or_else(|| anyhow::anyhow!("form already used"))?;
            let key = form.apply(&mut config.providers[index]);
            Ok((config.providers[index].id.clone(), key))
        })
        .map_err(fail)?;
    if let Some(key) = key {
        secrets::set(&provider_id, &key).map_err(fail)?;
    }
    refresh_tray(&app);
    Ok(())
}

#[tauri::command(async)]
fn remove_provider(app: AppHandle, store: State<Store>, id: String) -> Reply<()> {
    ledger::remove_provider(&store, &id, false).map_err(fail)?;
    refresh_tray(&app);
    Ok(())
}

fn launch_provider(
    store: &Store,
    id: &str,
    tool: Option<Tool>,
    model: Option<String>,
) -> Reply<()> {
    let config = store.load().map_err(fail)?;
    let provider = config.provider(id).map_err(fail)?;
    let tool = tool.unwrap_or(provider.tools()[0]);
    let model = model.and_then(clearable);
    let home = directories_home();
    terminal::open_provider(
        provider,
        tool,
        model.as_deref(),
        &config.terminal,
        home.as_deref(),
    )
    .map_err(fail)
}

#[tauri::command(async)]
fn open_provider(store: State<Store>, id: String, tool: Tool, model: Option<String>) -> Reply<()> {
    launch_provider(&store, &id, Some(tool), model)
}

fn meter(name: &str, window: &Option<Window>) -> Option<String> {
    window
        .as_ref()
        .map(|window| format!("{name} {:.0}%", window.percent))
}

fn tray_line(row: &Row) -> String {
    let limits = row.limits.clone().unwrap_or_default();
    let meters: Vec<String> = [meter("5h", &limits.session), meter("7d", &limits.weekly)]
        .into_iter()
        .flatten()
        .collect();
    let needs_login =
        row.account.is_login() && row.status.as_ref().is_some_and(|status| !status.logged_in);
    match (needs_login, meters.is_empty()) {
        (true, _) => format!("{}  ·  sign in again", row.account.label),
        (false, true) => row.account.label.clone(),
        (false, false) => format!("{}  ·  {}", row.account.label, meters.join("  ")),
    }
}

fn refresh_tray(app: &AppHandle) {
    let store = app.state::<Store>();
    let rows = snapshot(&store, 60).unwrap_or_default();
    let mut menu = MenuBuilder::new(app);
    for row in &rows {
        if let Ok(item) =
            MenuItemBuilder::with_id(format!("open:{}", row.account.id), tray_line(row)).build(app)
        {
            menu = menu.item(&item);
        }
    }
    if !rows.is_empty() {
        menu = menu.separator();
    }
    let keys = store
        .load()
        .map(|config| config.providers)
        .unwrap_or_default();
    for provider in &keys {
        if let Ok(item) =
            MenuItemBuilder::with_id(format!("use:{}", provider.id), &provider.label).build(app)
        {
            menu = menu.item(&item);
        }
    }
    if !keys.is_empty() {
        menu = menu.separator();
    }
    let built = menu
        .text("show", "Open Versatile Vault")
        .text("quit", "Quit")
        .build();
    if let (Ok(menu), Some(tray)) = (built, app.tray_by_id(TRAY_ID)) {
        let _ = tray.set_menu(Some(menu));
    }
    let _ = app.emit("ledger-changed", ());
}

fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn background(app: AppHandle) {
    std::thread::spawn(move || {
        let mut tick = 0u32;
        loop {
            if tick.is_multiple_of(KEEPALIVE_EVERY) {
                let store = app.state::<Store>();
                if let Ok(config) = store.load() {
                    status::keepalive(&store, &config, false);
                }
            }
            refresh_tray(&app);
            tick = tick.wrapping_add(1);
            std::thread::sleep(REFRESH);
        }
    });
}

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    let store = match Store::open() {
        Ok(store) => store,
        Err(error) => {
            eprintln!("vvault: {error:#}");
            std::process::exit(1);
        }
    };
    tauri::Builder::default()
        .manage(store)
        .manage(Logins::default())
        .invoke_handler(tauri::generate_handler![
            accounts,
            open_account,
            login_account,
            login_code,
            login_reopen,
            login_cancel,
            account_models,
            harness_models,
            key_models,
            provider_models,
            add_account,
            adopt_account,
            update_account,
            set_key,
            remove_account,
            settings,
            save_settings,
            providers,
            provider_presets,
            add_provider,
            update_provider,
            remove_provider,
            open_provider
        ])
        .setup(|app| {
            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(icon)
                .icon_as_template(true)
                .tooltip("Versatile Vault")
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => show(app),
                    "quit" => app.exit(0),
                    id => {
                        if let Some(account) = id.strip_prefix("open:") {
                            let _ = open_in_terminal(&app.state::<Store>(), account, "run");
                        }
                        if let Some(provider) = id.strip_prefix("use:") {
                            let _ = launch_provider(&app.state::<Store>(), provider, None, None);
                        }
                    }
                })
                .build(app)?;
            background(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to start Versatile Vault");
}
