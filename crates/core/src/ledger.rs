use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::limits::{self, Limits};
use crate::model::{Account, Auth, Config, Harness, Provider};
use crate::secrets;
use crate::status::{self, Status};
use crate::store::Store;

/// Everything needed to register an account.
pub struct NewAccount {
    pub label: String,
    pub harness: Harness,
    pub auth: Auth,
    pub model: Option<String>,
    pub fast_model: Option<String>,
    pub key: Option<String>,
}

/// An account with its last known limits and session state.
#[derive(Clone, Debug, Serialize)]
pub struct Row {
    #[serde(flatten)]
    pub account: Account,
    pub limits: Option<Limits>,
    pub status: Option<Status>,
}

/// Trims an optional text field, mapping blank to absent.
pub fn clearable(value: String) -> Option<String> {
    Some(value.trim().to_string()).filter(|text| !text.is_empty())
}

fn label_taken(config: &Config, label: &str) -> bool {
    config
        .accounts
        .iter()
        .any(|other| other.label.eq_ignore_ascii_case(label))
}

/// Registers an account; an API key goes to the OS keychain, never to the ledger.
pub fn create(store: &Store, new: NewAccount) -> Result<Account> {
    let mut account = Account::new(&new.label, new.harness, new.auth)?;
    account.model = new.model.and_then(clearable);
    account.fast_model = new.fast_model.and_then(clearable);
    account.validate()?;
    if label_taken(&store.load()?, &account.label) {
        bail!("an account named `{}` already exists", account.label);
    }
    match (&account.auth, &new.key) {
        (Auth::ApiKey { .. }, Some(key)) => secrets::set(&account.id, key)?,
        (Auth::ApiKey { .. }, None) => bail!("an API key is required for this account"),
        (Auth::Login, _) => {}
    }
    let saved = store.update(|config| {
        if label_taken(config, &account.label) {
            bail!("an account named `{}` already exists", account.label);
        }
        config.accounts.push(account.clone());
        Ok(())
    });
    if saved.is_err() {
        let _ = secrets::delete(&account.id);
    }
    saved.map(|_| account)
}

/// Tracks an existing harness home in place; nothing is copied or moved.
pub fn adopt(
    store: &Store,
    label: &str,
    harness: Harness,
    dir: Option<PathBuf>,
) -> Result<Account> {
    let native = harness
        .native_home()
        .context("could not resolve the home directory")?;
    let home = match dir {
        Some(path) => path
            .canonicalize()
            .with_context(|| format!("{} does not exist", path.display()))?,
        None => native.clone(),
    };
    if !home.is_dir() {
        bail!("{} is not a directory", home.display());
    }
    let mut account = Account::new(label, harness, Auth::Login)?;
    account.native_home = home == native || native.canonicalize().is_ok_and(|path| path == home);
    account.home = Some(home.clone());
    store.update(|config| {
        if label_taken(config, &account.label)
            || config
                .accounts
                .iter()
                .any(|other| store.home(other) == home)
        {
            bail!("that label or directory is already in the ledger");
        }
        config.accounts.push(account.clone());
        Ok(())
    })?;
    let _ = status::check(store, &account);
    Ok(account)
}

/// Removes an account and its key; returns it and whether its managed profile was deleted.
pub fn remove(store: &Store, key: &str, purge: bool) -> Result<(Account, bool)> {
    let removed = store.update(|config| {
        let index = config.position(key)?;
        Ok(config.accounts.remove(index))
    })?;
    let _ = secrets::delete(&removed.id);
    for (kind, extension) in [("limits", "json"), ("status", "json"), ("status", "lock")] {
        if let Ok(path) = store.cache(kind, &removed.id, extension) {
            let _ = std::fs::remove_file(path);
        }
    }
    let home = store.home(&removed);
    let purged = purge
        && removed.home.is_none()
        && home.starts_with(store.root().join("profiles"))
        && std::fs::remove_dir_all(&home).is_ok();
    Ok((removed, purged))
}

/// Registers a provider; its key goes to the OS keychain, never to the ledger.
pub fn create_provider(store: &Store, provider: Provider, key: &str) -> Result<Provider> {
    provider.validate()?;
    let taken = |config: &Config| config.providers.iter().any(|other| other.label.eq_ignore_ascii_case(&provider.label));
    if taken(&store.load()?) {
        bail!("a provider named `{}` already exists", provider.label);
    }
    secrets::set(&provider.id, key)?;
    let saved = store.update(|config| {
        if taken(config) {
            bail!("a provider named `{}` already exists", provider.label);
        }
        config.providers.push(provider.clone());
        Ok(())
    });
    if saved.is_err() {
        let _ = secrets::delete(&provider.id);
    }
    saved.map(|_| provider)
}

/// Removes a provider and its key; returns it and whether its profiles were deleted.
pub fn remove_provider(store: &Store, key: &str, purge: bool) -> Result<(Provider, bool)> {
    let removed = store.update(|config| {
        let index = config.provider_position(key)?;
        Ok(config.providers.remove(index))
    })?;
    let _ = secrets::delete(&removed.id);
    let home = store.root().join("profiles").join(&removed.id);
    let purged = purge && home.is_dir() && std::fs::remove_dir_all(&home).is_ok();
    Ok((removed, purged))
}

/// Every account with limits no older than `max_age` seconds where they can be probed.
pub fn rows(store: &Store, config: &Config, max_age: i64) -> Vec<Row> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = config
            .accounts
            .iter()
            .map(|account| scope.spawn(move || limits::current(store, account, max_age)))
            .collect();
        config
            .accounts
            .iter()
            .zip(handles)
            .map(|(account, handle)| Row {
                account: account.clone(),
                limits: handle.join().ok().flatten(),
                status: status::read(store, &account.id),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::at(directory.path().join("ledger")).unwrap();
        (directory, store)
    }

    fn login(label: &str) -> NewAccount {
        NewAccount {
            label: label.into(),
            harness: Harness::Claude,
            auth: Auth::Login,
            model: Some("  ".into()),
            fast_model: None,
            key: None,
        }
    }

    #[test]
    fn creates_login_accounts_and_rejects_duplicate_labels() {
        let (_guard, store) = store();
        let account = create(&store, login("Work")).unwrap();
        assert_eq!(account.model, None);
        assert!(create(&store, login("work")).is_err());
        assert_eq!(store.load().unwrap().accounts.len(), 1);
    }

    #[test]
    fn refuses_an_api_key_account_without_a_key() {
        let (_guard, store) = store();
        let new = NewAccount {
            auth: Auth::ApiKey {
                base_url: None,
                bearer: false,
            },
            ..login("api")
        };
        assert!(create(&store, new).is_err());
        assert!(store.load().unwrap().accounts.is_empty());
    }

    #[test]
    fn purging_deletes_only_managed_profiles() {
        let (guard, store) = store();
        let managed = create(&store, login("managed")).unwrap();
        let managed_home = store.ensure_home(&managed).unwrap();
        let foreign = guard.path().join("foreign");
        std::fs::create_dir_all(&foreign).unwrap();
        store
            .update(|config| {
                let mut account = Account::new("adopted", Harness::Claude, Auth::Login)?;
                account.home = Some(foreign.clone());
                config.accounts.push(account);
                Ok(())
            })
            .unwrap();
        assert!(remove(&store, "managed", true).unwrap().1);
        assert!(!remove(&store, "adopted", true).unwrap().1);
        assert!(!managed_home.exists());
        assert!(foreign.is_dir());
    }
}
