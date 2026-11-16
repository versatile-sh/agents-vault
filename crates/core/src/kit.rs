use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::launch::scrubbed;
use crate::model::{Account, Config, Harness};
use crate::store::{write_json, Store};

fn link(source: &Path, target: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target)?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{symlink_dir, symlink_file};
        if source.is_dir() {
            symlink_dir(source, target)
                .context("linking a folder needs Windows Developer Mode enabled")?;
        } else if symlink_file(source, target).is_err() {
            fs::copy(source, target)?;
        }
    }
    Ok(())
}

fn seed(harness: Harness, source: &Path, target: &Path) -> Result<()> {
    if harness != Harness::Claude {
        fs::copy(source, target)?;
        return Ok(());
    }
    let raw = fs::read_to_string(source)?;
    let mut settings: serde_json::Value =
        serde_json::from_str(&raw).context("kit settings.json is not valid JSON")?;
    if let Some(object) = settings.as_object_mut() {
        object.remove("apiKeyHelper");
        if let Some(env) = object.get_mut("env").and_then(|env| env.as_object_mut()) {
            env.retain(|key, _| !scrubbed(key));
        }
    }
    write_json(target, &settings)
}

/// Links the shared kit into the account home; returns one warning per skipped entry.
pub fn apply(store: &Store, config: &Config, account: &Account) -> Result<Vec<String>> {
    let home = store.ensure_home(account)?;
    let kit = config.kit(account.harness);
    let Some(directory) = kit
        .dir
        .as_deref()
        .filter(|_| account.kit && !account.native_home)
    else {
        return Ok(Vec::new());
    };
    let same = match (directory.canonicalize(), home.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if same || !directory.is_dir() {
        return Ok(Vec::new());
    }
    let defaults: Vec<String> = account
        .harness
        .default_kit_entries()
        .iter()
        .map(|e| e.to_string())
        .collect();
    let entries = if kit.entries.is_empty() {
        &defaults
    } else {
        &kit.entries
    };
    let mut warnings = Vec::new();
    for name in entries {
        let plain = !name.is_empty() && !name.contains(['/', '\\']) && name != ".." && name != ".";
        if !plain {
            warnings.push(format!(
                "kit entry `{name}` ignored: it must be a plain name"
            ));
            continue;
        }
        let source = directory.join(name);
        let target = home.join(name);
        if !source.exists() {
            continue;
        }
        if target.is_symlink() {
            if fs::read_link(&target).is_ok_and(|current| current == source) {
                continue;
            }
            let removed = fs::remove_file(&target).or_else(|_| fs::remove_dir(&target));
            if let Err(error) = removed {
                warnings.push(format!("kit entry `{name}` not relinked: {error}"));
                continue;
            }
        } else if target.exists() {
            continue;
        }
        if let Err(error) = link(&source, &target) {
            warnings.push(format!("kit entry `{name}` not linked: {error:#}"));
        }
    }
    let file = account.harness.seed_file();
    let (source, target) = (directory.join(file), home.join(file));
    if source.is_file() && !target.exists() {
        if let Err(error) = seed(account.harness, &source, &target) {
            warnings.push(format!("{file} not seeded: {error:#}"));
        }
    }
    Ok(warnings)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::model::Auth;

    fn fixture() -> (tempfile::TempDir, Store, Config, Account) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::at(directory.path().join("ledger")).unwrap();
        let kit = directory.path().join("kit");
        fs::create_dir_all(kit.join("skills")).unwrap();
        fs::write(kit.join("CLAUDE.md"), "rules").unwrap();
        fs::write(
            kit.join("settings.json"),
            r#"{"apiKeyHelper":"x","model":"opus","env":{"ANTHROPIC_API_KEY":"k","KEEP":"1"}}"#,
        )
        .unwrap();
        let mut config = Config::default();
        config.claude_kit.dir = Some(kit);
        let account = Account::new("work", Harness::Claude, Auth::Login).unwrap();
        (directory, store, config, account)
    }

    #[test]
    fn links_instructions_and_seeds_sanitized_settings() {
        let (_guard, store, config, account) = fixture();
        assert!(apply(&store, &config, &account).unwrap().is_empty());
        let home = store.home(&account);
        assert_eq!(fs::read_to_string(home.join("CLAUDE.md")).unwrap(), "rules");
        assert!(home.join("skills").is_symlink());
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(home.join("settings.json")).unwrap()).unwrap();
        assert_eq!(
            settings,
            serde_json::json!({"model": "opus", "env": {"KEEP": "1"}})
        );
    }

    #[test]
    fn never_replaces_files_the_user_owns_in_the_profile() {
        let (_guard, store, config, account) = fixture();
        let home = store.home(&account);
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("CLAUDE.md"), "mine").unwrap();
        fs::write(home.join("settings.json"), "{}").unwrap();
        apply(&store, &config, &account).unwrap();
        assert_eq!(fs::read_to_string(home.join("CLAUDE.md")).unwrap(), "mine");
        assert_eq!(
            fs::read_to_string(home.join("settings.json")).unwrap(),
            "{}"
        );
    }

    #[test]
    fn repoints_its_own_links_when_the_kit_moves() {
        let (guard, store, mut config, account) = fixture();
        apply(&store, &config, &account).unwrap();
        let moved = guard.path().join("kit2");
        fs::create_dir_all(&moved).unwrap();
        fs::write(moved.join("CLAUDE.md"), "new rules").unwrap();
        config.claude_kit.dir = Some(moved);
        apply(&store, &config, &account).unwrap();
        assert_eq!(
            fs::read_to_string(store.home(&account).join("CLAUDE.md")).unwrap(),
            "new rules"
        );
    }

    #[test]
    fn leaves_accounts_alone_when_the_kit_is_off_or_escapes_its_folder() {
        let (_guard, store, mut config, mut account) = fixture();
        config.claude_kit.entries = vec!["../outside".into()];
        let warnings = apply(&store, &config, &account).unwrap();
        assert_eq!(warnings.len(), 1);
        account.kit = false;
        config.claude_kit.entries.clear();
        apply(&store, &config, &account).unwrap();
        assert!(!store.home(&account).join("CLAUDE.md").exists());
    }
}
