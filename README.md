# Cofre

One ledger for every Claude Code and Codex account, API key and router. Click an account and a terminal opens already signed in, on the right model, with your shared instructions in place. A tray icon shows each account's 5-hour and weekly limits.

Works on Linux, macOS and Windows. Not affiliated with Anthropic or OpenAI.

## How it stays safe

| Rule | What it means |
| --- | --- |
| One home per account | Every account runs with its own `CLAUDE_CONFIG_DIR` or `CODEX_HOME`. Credentials are never copied between folders, so a rotating refresh token only ever exists in one place. |
| The harness owns its tokens | Cofre never reads, stores or refreshes an OAuth token. Sign-in, refresh and sign-out are done by `claude` and `codex` themselves, inside the account's home. |
| Keys live in the OS keychain | API keys go to macOS Keychain, Windows Credential Manager or the Secret Service. They are not written to the ledger, to a config file or to a command line. |
| Keys are resolved late | The terminal runs `cofre run <id>`; the key is read from the keychain there and passed to the harness through its environment only. |
| Clean environment | Variables that would override an identity (`ANTHROPIC_*`, `CODEX_HOME`, `OPENAI_API_KEY`, …) are removed before the harness starts. |
| Clear text is refused | A custom endpoint must be `https://`; plain `http://` is accepted only for localhost. |

The ledger (`ledger.json`) holds labels, models and folder paths. It is created readable only by you.

## Install

Requires Rust 1.89+ and, for the tray app, the [Tauri 2 system dependencies](https://tauri.app/start/prerequisites/).

```sh
cargo install --path crates/cli
cargo build --release -p cofre-app --features custom-protocol
```

Put `cofre-app` next to `cofre` (or keep `cofre` on `PATH`). For installers (`.dmg`, `.msi`, `.deb`, `.AppImage`), run `cargo tauri build` inside `app/src-tauri`.

## Use

```sh
cofre add work                       # opens the official Claude Code sign-in
cofre add team --harness codex       # opens the official Codex sign-in
cofre add router --base-url https://openrouter.ai/api --bearer --model vendor/model
cofre add api --api-key --model claude-sonnet-5-5
cofre adopt personal                 # track your existing ~/.claude in place
cofre adopt old --dir ~/some/profile # track any existing home, nothing is copied

cofre list                           # accounts, limits and session state
cofre open work                      # new terminal window on that account
cofre run work -- --resume           # same, in the current terminal
cofre set work --model opus          # model every session starts with
cofre key router                     # replace a key in the keychain
cofre remove work                    # the profile folder is kept unless --purge
```

### Shared instructions

Point Cofre at one folder and every managed account links to it: `CLAUDE.md`, `skills`, `commands`, `agents`, `rules` for Claude Code; `AGENTS.md`, `skills`, `prompts` for Codex. `settings.json` / `config.toml` are copied once into new profiles, with credential settings stripped.

```sh
cofre kit claude --dir ~/.claude
cofre kit codex --dir ~/.codex
```

Files you already have inside a profile are never replaced.

### Limits

| Account | Source | Freshness |
| --- | --- | --- |
| Claude Code sign-in | The `rate_limits` field Claude Code sends to its status line | Updated on every reply of a running session; windows reset on their own when their time passes |
| Codex sign-in | The Codex CLI's own `account/rateLimits/read` request | On demand, cached for 60 seconds |
| API key / router | None; these are pay as you go | — |

Cofre adds its status line through `--settings` at launch, so your `settings.json` is not modified. If you already have a status line, it keeps running and its output is what you see.

### Keeping sessions alive

The tray app renews idle accounts in the background by asking each harness for its own status. Without the app, schedule the same thing:

```sh
cofre keepalive        # exit code 1 when an account needs a new sign-in
```

### Bars and widgets

```sh
cofre list --json
cofre waybar           # {"text": "37%", "tooltip": "…", "class": "ok|warning|critical"}
```

## Layout

| Path | Purpose |
| --- | --- |
| `crates/core` | Ledger, launcher, keychain, limits, keep-alive |
| `crates/cli` | The `cofre` command |
| `app/src-tauri` | Tray app backend |
| `app/ui` | Tray app interface (plain HTML, CSS and JS, no build step) |
| `scripts/icons.py` | Regenerates the icons |

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
COFRE_HOME=/tmp/cofre-dev cargo run -p cofre -- list
```

`COFRE_HOME` moves the ledger, which keeps experiments away from your real accounts.

## License

MIT
