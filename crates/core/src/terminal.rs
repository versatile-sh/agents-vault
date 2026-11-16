use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::model::{Account, Provider, Tool};

/// Path of the `vvault` CLI: `VVAULT_CLI`, a sibling of this executable, or PATH.
pub fn cli_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("VVAULT_CLI").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "vvault.exe"
    } else {
        "vvault"
    };
    let current = std::env::current_exe().context("could not locate this executable")?;
    if current.file_name().is_some_and(|file| file == name) {
        return Ok(current);
    }
    let sibling = current.with_file_name(name);
    if sibling.is_file() {
        return Ok(sibling);
    }
    which::which("vvault").context("the `vvault` CLI was not found next to the app or on PATH")
}

fn candidates(title: &str) -> Vec<(String, Vec<String>)> {
    let owned = |items: &[&str]| {
        items
            .iter()
            .map(|item| item.to_string())
            .collect::<Vec<_>>()
    };
    if cfg!(target_os = "windows") {
        return vec![
            ("wt.exe".into(), owned(&["new-tab", "--title", title, "--"])),
            ("cmd.exe".into(), owned(&["/C", "start", title])),
        ];
    }
    let mut list = Vec::new();
    if let Ok(terminal) = std::env::var("TERMINAL") {
        if !terminal.trim().is_empty() {
            list.push((terminal.trim().to_string(), owned(&["-e"])));
        }
    }
    list.extend([
        ("xdg-terminal-exec".to_string(), owned(&[])),
        ("kitty".to_string(), owned(&["--title", title])),
        ("ghostty".to_string(), owned(&["-e"])),
        ("alacritty".to_string(), owned(&["--title", title, "-e"])),
        ("foot".to_string(), owned(&["--title", title])),
        ("wezterm".to_string(), owned(&["start", "--"])),
        ("konsole".to_string(), owned(&["-e"])),
        ("gnome-terminal".to_string(), owned(&["--"])),
        ("xfce4-terminal".to_string(), owned(&["-x"])),
        ("xterm".to_string(), owned(&["-e"])),
    ]);
    list
}

fn apple_script(cli: &Path, arguments: &[&str]) -> String {
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
    let mut line = quote(&cli.to_string_lossy());
    for argument in arguments {
        line.push(' ');
        line.push_str(&quote(argument));
    }
    let escaped = line.replace('\\', "\\\\").replace('"', "\\\"");
    format!("tell application \"Terminal\"\nactivate\ndo script \"{escaped}; exit\"\nend tell")
}

fn spawn(mut command: Command, directory: Option<&Path>) -> Result<()> {
    if let Some(directory) = directory.filter(|path| path.is_dir()) {
        command.current_dir(directory);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .spawn()
        .map(drop)
        .context("could not open a terminal window")
}

/// Opens a terminal window running `vvault <arguments>`; secrets are resolved inside it.
pub fn open(
    arguments: &[String],
    title: &str,
    custom: &[String],
    directory: Option<&Path>,
) -> Result<()> {
    let cli = cli_path()?;
    if let Some((program, prefix)) = custom.split_first() {
        let mut command = Command::new(program);
        command.args(prefix).arg(&cli).args(arguments);
        return spawn(command, directory);
    }
    if cfg!(target_os = "macos") {
        let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let mut command = Command::new("osascript");
        command.arg("-e").arg(apple_script(&cli, &borrowed));
        return spawn(command, directory);
    }
    for (program, prefix) in candidates(title) {
        if let Ok(path) = which::which(&program) {
            let mut command = Command::new(path);
            command.args(prefix).arg(&cli).args(arguments);
            return spawn(command, directory);
        }
    }
    bail!("no terminal emulator found; set one with `vvault terminal <program> [args]`")
}

/// Opens a terminal window on an account, running or signing it in.
pub fn open_account(
    account: &Account,
    action: &str,
    custom: &[String],
    directory: Option<&Path>,
) -> Result<()> {
    let arguments = [action.to_string(), account.id.clone(), "--hold".to_string()];
    let title = format!("{} · {}", account.harness.program(), account.label);
    open(&arguments, &title, custom, directory)
}

/// Opens a terminal window on a provider with the chosen tool and model.
pub fn open_provider(
    provider: &Provider,
    tool: Tool,
    model: Option<&str>,
    custom: &[String],
    directory: Option<&Path>,
) -> Result<()> {
    let name = match tool {
        Tool::Claude => "claude",
        Tool::Codex => "codex",
        Tool::Shell => "shell",
    };
    let mut arguments = vec![
        "use".to_string(),
        provider.id.clone(),
        "--with".to_string(),
        name.to_string(),
        "--hold".to_string(),
    ];
    if let Some(model) = model {
        arguments.extend(["--model".to_string(), model.to_string()]);
    }
    open(
        &arguments,
        &format!("{name} · {}", provider.label),
        custom,
        directory,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_script_survives_quotes_in_the_cli_path() {
        let script = apple_script(Path::new("/Apps/it's \"x\"/vvault"), &["run", "id"]);
        assert!(
            script.contains("do script \"'/Apps/it'\\\\''s \\\"x\\\"/vvault' 'run' 'id'; exit\"")
        );
    }
}
