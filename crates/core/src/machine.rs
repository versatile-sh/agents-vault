use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::model::{Account, Config, Harness};
use crate::store::Store;
use crate::terminal::cli_path;

const BEGIN: &str = "# >>> versatile vault >>>";
const END: &str = "# <<< versatile vault <<<";
const HARNESSES: [Harness; 3] = [Harness::Claude, Harness::Codex, Harness::Grok];

/// One environment variable to set, or to unset when the value is absent.
pub type Export = (String, Option<String>);

/// Makes a signed-in account the one every new terminal and app uses for its harness.
pub fn set_default(store: &Store, key: &str) -> Result<Account> {
    let account = store.update(|config| {
        let account = config.find(key)?.clone();
        if !account.is_login() {
            bail!("only signed-in accounts can be the machine default; an API key would have to sit in every shell");
        }
        config.defaults.insert(account.harness.program().to_string(), account.id.clone());
        Ok(account)
    })?;
    apply(store)?;
    Ok(account)
}

/// Stops overriding a harness, so it goes back to its own default home.
pub fn clear_default(store: &Store, harness: Harness) -> Result<()> {
    store.update(|config| {
        config.defaults.remove(harness.program());
        Ok(())
    })?;
    apply(store)
}

/// Variables that point each harness at its default account.
pub fn exports(store: &Store, config: &Config) -> Vec<Export> {
    HARNESSES
        .iter()
        .map(|harness| {
            let account = config
                .defaults
                .get(harness.program())
                .and_then(|id| config.accounts.iter().find(|account| &account.id == id && account.harness == *harness));
            let home = account
                .filter(|account| !account.native_home)
                .map(|account| store.home(account).to_string_lossy().into_owned());
            (harness.home_env().to_string(), home)
        })
        .collect()
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Shell code that applies the exports; `fish` and `powershell` get their own syntax.
pub fn snippet(shell: &str, exports: &[Export]) -> String {
    exports
        .iter()
        .map(|(name, value)| match (shell, value) {
            ("fish", Some(value)) => format!("set -gx {name} {}\n", quote(value)),
            ("fish", None) => format!("set -e {name}\n"),
            ("powershell", Some(value)) => format!("$env:{name} = '{}'\n", value.replace('\'', "''")),
            ("powershell", None) => format!("Remove-Item Env:{name} -ErrorAction SilentlyContinue\n"),
            (_, Some(value)) => format!("export {name}={}\n", quote(value)),
            (_, None) => format!("unset {name}\n"),
        })
        .collect()
}

fn block(shell: &str, cli: &Path) -> String {
    let cli = quote(&cli.to_string_lossy());
    let line = match shell {
        "fish" => format!("test -x {cli}; and {cli} env --shell fish | source"),
        _ => format!("[ -x {cli} ] && eval \"$({cli} env --shell {shell})\""),
    };
    format!("{BEGIN}\n{line}\n{END}\n")
}

/// Replaces the Vault block of a shell profile, or appends it when absent.
pub fn with_block(profile: &str, block: &str) -> String {
    match (profile.find(BEGIN), profile.find(END)) {
        (Some(start), Some(end)) if end > start => {
            let after = profile[end + END.len()..].trim_start_matches('\n');
            format!("{}{block}{after}", &profile[..start])
        }
        _ if profile.is_empty() || profile.ends_with("\n\n") => format!("{profile}{block}"),
        _ => format!("{}\n\n{block}", profile.trim_end_matches('\n')),
    }
}

fn home() -> Result<PathBuf> {
    Ok(directories::BaseDirs::new().context("could not resolve the home directory")?.home_dir().to_path_buf())
}

fn install_hooks(home: &Path, cli: &Path) -> Result<()> {
    for (shell, file) in [("bash", ".bashrc"), ("zsh", ".zshrc")] {
        let path = home.join(file);
        if let Ok(current) = std::fs::read_to_string(&path) {
            let updated = with_block(&current, &block(shell, cli));
            if updated != current {
                std::fs::write(&path, updated).with_context(|| format!("writing {}", path.display()))?;
            }
        }
    }
    let fish = home.join(".config/fish/conf.d");
    if fish.is_dir() {
        std::fs::write(fish.join("vvault.fish"), block("fish", cli))?;
    }
    Ok(())
}

fn session_environment(home: &Path, exports: &[Export]) -> Result<()> {
    if cfg!(target_os = "windows") {
        for (name, value) in exports {
            let mut command = Command::new("setx");
            command.arg(name).arg(value.as_deref().unwrap_or(""));
            let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
        return Ok(());
    }
    if cfg!(target_os = "macos") {
        for (name, value) in exports {
            let mut command = Command::new("launchctl");
            match value {
                Some(value) => command.args(["setenv", name, value]),
                None => command.args(["unsetenv", name]),
            };
            let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
        return Ok(());
    }
    let directory = home.join(".config/environment.d");
    let path = directory.join("60-vvault.conf");
    let lines: String = exports
        .iter()
        .filter_map(|(name, value)| Some(format!("{name}={}\n", value.as_ref()?)))
        .collect();
    if lines.is_empty() {
        let _ = std::fs::remove_file(path);
        return Ok(());
    }
    std::fs::create_dir_all(&directory)?;
    std::fs::write(&path, lines).with_context(|| format!("writing {}", path.display()))
}

/// Writes the shell hook and the session environment for the current defaults.
pub fn apply(store: &Store) -> Result<()> {
    let config = store.load()?;
    let home = home()?;
    install_hooks(&home, &cli_path()?)?;
    session_environment(&home, &exports(store, &config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Auth;

    fn fixture() -> (tempfile::TempDir, Store, Config) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::at(directory.path().join("ledger")).unwrap();
        let mut config = Config::default();
        let managed = Account::new("work", Harness::Claude, Auth::Login).unwrap();
        let mut native = Account::new("main", Harness::Codex, Auth::Login).unwrap();
        native.native_home = true;
        native.home = Harness::Codex.native_home();
        config.defaults.insert("claude".into(), managed.id.clone());
        config.defaults.insert("codex".into(), native.id.clone());
        config.accounts = vec![managed, native];
        (directory, store, config)
    }

    #[test]
    fn points_each_harness_at_its_default_and_unsets_the_rest() {
        let (_guard, store, config) = fixture();
        let exports = exports(&store, &config);
        assert_eq!(exports[0].0, "CLAUDE_CONFIG_DIR");
        assert!(exports[0].1.as_ref().unwrap().ends_with(&config.accounts[0].id));
        assert_eq!(exports[1], ("CODEX_HOME".to_string(), None));
        assert_eq!(exports[2], ("GROK_HOME".to_string(), None));
    }

    #[test]
    fn writes_each_shell_in_its_own_syntax() {
        let exports = vec![("A_HOME".to_string(), Some("/it's".to_string())), ("B_HOME".to_string(), None)];
        assert_eq!(snippet("bash", &exports), "export A_HOME='/it'\\''s'\nunset B_HOME\n");
        assert_eq!(snippet("fish", &exports), "set -gx A_HOME '/it'\\''s'\nset -e B_HOME\n");
        assert!(snippet("powershell", &exports).starts_with("$env:A_HOME = '/it''s'\n"));
    }

    #[test]
    fn keeps_one_block_in_a_profile_however_often_it_runs() {
        let first = with_block("alias l=ls\n", "# >>> versatile vault >>>\none\n# <<< versatile vault <<<\n");
        assert_eq!(first, "alias l=ls\n\n# >>> versatile vault >>>\none\n# <<< versatile vault <<<\n");
        let second = with_block(&format!("{first}export X=1\n"), "# >>> versatile vault >>>\ntwo\n# <<< versatile vault <<<\n");
        assert_eq!(second, "alias l=ls\n\n# >>> versatile vault >>>\ntwo\n# <<< versatile vault <<<\nexport X=1\n");
    }

    #[test]
    fn installs_hooks_only_in_profiles_that_exist() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(".bashrc"), "# mine\n").unwrap();
        install_hooks(directory.path(), Path::new("/opt/vvault")).unwrap();
        let bashrc = std::fs::read_to_string(directory.path().join(".bashrc")).unwrap();
        assert!(bashrc.starts_with("# mine\n"));
        assert!(bashrc.contains("eval \"$('/opt/vvault' env --shell bash)\""));
        assert!(!directory.path().join(".zshrc").exists());
    }
}
