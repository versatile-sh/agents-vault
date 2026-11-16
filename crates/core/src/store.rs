use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::model::{Account, Config};

/// On-disk ledger root: the account list, managed profiles and cached state.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Opens the ledger at `VVAULT_HOME` or the platform data directory.
    pub fn open() -> Result<Self> {
        let root = match std::env::var_os("VVAULT_HOME") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => directories::ProjectDirs::from("sh", "versatile", "vvault")
                .context("could not resolve a data directory for this user")?
                .data_dir()
                .to_path_buf(),
        };
        Self::at(root)
    }

    /// Opens the ledger at an explicit root, creating it private to the user.
    pub fn at(root: PathBuf) -> Result<Self> {
        private_dir(&root)?;
        Ok(Self { root })
    }

    /// Directory holding everything the ledger persists.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads the ledger, or an empty one when nothing was saved yet.
    pub fn load(&self) -> Result<Config> {
        let path = self.root.join("ledger.json");
        if !path.exists() {
            return Ok(Config::default());
        }
        let raw =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("{} is not a valid ledger", path.display()))
    }

    /// Applies a change under an exclusive lock and saves it atomically.
    pub fn update<T>(&self, change: impl FnOnce(&mut Config) -> Result<T>) -> Result<T> {
        let lock = File::options()
            .create(true)
            .append(true)
            .open(self.root.join(".lock"))?;
        lock.lock()?;
        let mut config = self.load()?;
        let output = change(&mut config)?;
        for account in &config.accounts {
            account.validate()?;
        }
        for provider in &config.providers {
            provider.validate()?;
        }
        write_json(&self.root.join("ledger.json"), &config)?;
        Ok(output)
    }

    /// Home directory the harness uses for this account.
    pub fn home(&self, account: &Account) -> PathBuf {
        account
            .home
            .clone()
            .unwrap_or_else(|| self.root.join("profiles").join(&account.id))
    }

    /// Creates a profile directory private to the user; homes outside the ledger are left alone.
    pub fn ensure_home(&self, account: &Account) -> Result<PathBuf> {
        let home = self.home(account);
        let profiles = self.root.join("profiles");
        if home.starts_with(&profiles) {
            private_dir(&profiles)?;
            private_dir(&home)?;
        }
        Ok(home)
    }

    /// Path of a per-account cache file such as `limits/<id>.json`.
    pub fn cache(&self, kind: &str, id: &str, extension: &str) -> Result<PathBuf> {
        uuid::Uuid::parse_str(id).context("invalid account id")?;
        let directory = self.root.join(kind);
        private_dir(&directory)?;
        Ok(directory.join(format!("{id}.{extension}")))
    }
}

/// Creates a directory readable only by the current user.
pub fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes JSON through a private temporary file and an atomic rename.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().context("path has no parent directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Reads JSON, treating a missing or malformed file as absent.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Current Unix time in seconds.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Auth, Harness};

    #[test]
    fn saves_and_reloads_accounts() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::at(directory.path().join("ledger")).unwrap();
        let id = store
            .update(|config| {
                let account = Account::new("personal", Harness::Codex, Auth::Login)?;
                config.accounts.push(account.clone());
                Ok(account.id)
            })
            .unwrap();
        let config = store.load().unwrap();
        assert_eq!(config.find("personal").unwrap().id, id);
        assert_eq!(
            store.home(&config.accounts[0]),
            store.root().join("profiles").join(&id)
        );
    }

    #[test]
    fn refuses_to_save_an_invalid_account() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::at(directory.path().to_path_buf()).unwrap();
        let result = store.update(|config| {
            let mut account = Account::new("x", Harness::Claude, Auth::Login)?;
            account.label = String::new();
            config.accounts.push(account);
            Ok(())
        });
        assert!(result.is_err());
        assert!(store.load().unwrap().accounts.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_ledger_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let store = Store::at(directory.path().join("ledger")).unwrap();
        store.update(|_| Ok(())).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(store.root()), 0o700);
        assert_eq!(mode(&store.root().join("ledger.json")), 0o600);
    }
}
