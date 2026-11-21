# Using vvault

In the app, signing in opens the official page in your browser and finishes by itself; no terminal is involved. From the command line:

```sh
vvault add work                       # runs the official Claude Code sign-in
vvault add team --harness codex       # runs the official Codex sign-in
vvault add x --harness grok           # runs the official Grok sign-in
vvault add router --base-url https://openrouter.ai/api --bearer --model vendor/model
vvault add api --api-key --model claude-sonnet-5-5
vvault adopt personal                 # track your existing ~/.claude in place
vvault adopt old --dir ~/some/profile # track any existing home, nothing is copied

vvault list                           # accounts, limits and session state
vvault open work                      # new terminal window on that account
vvault run work -- --resume           # same, in the current terminal
vvault set work --model opus          # model every session starts with
vvault key router                     # replace a key in the keychain
vvault remove work                    # the profile folder is kept unless --purge
```

## One account for the whole machine

"Use on this machine" in the app, or `vvault default`, makes plain `claude`, `codex` or `grok` use that account in every new terminal and app.

```sh
vvault default work            # plain `claude` now means the work account
vvault default --clear claude  # back to the harness's own default home
vvault default                 # show the current defaults
```

It works by pointing the harness at the account's home, so nothing is copied and there is still one copy of the session.

| Where | What is written |
| --- | --- |
| `~/.bashrc`, `~/.zshrc`, `~/.config/fish/conf.d/vvault.fish` (only those that exist) | One marked block that runs `vvault env` when a shell starts |
| Linux: `~/.config/environment.d/60-vvault.conf` | The variable for graphical apps, read at the next login |
| macOS: `launchctl setenv` | The variable for apps started afterwards |
| Windows: `setx` | The user variable for programs started afterwards |

Terminals that are already open keep their account until you run `eval "$(vvault env)"` or reopen them. Only signed-in accounts can be the default, because an API key would have to sit in every shell.

## API keys for other providers

A provider key is stored once and opened in whatever tool you use. Presets fill the endpoints for OpenRouter and DeepSeek; any compatible endpoint works with `--anthropic-url` and `--openai-url`.

```sh
vvault provider add OpenRouter --preset openrouter --model vendor/model-a --model vendor/model-b
vvault provider add DeepSeek --preset deepseek --model deepseek-chat --command opencode
vvault provider add Gateway --key-env GATEWAY_API_KEY --openai-url https://gateway.example/v1

vvault use OpenRouter                         # Claude Code on the provider's endpoint, first model
vvault use OpenRouter --with codex --model vendor/model-b
vvault use DeepSeek --with shell              # runs the provider's tool, or your shell, with the key exported
vvault use OpenRouter --window                # same, in a new terminal window
```

| `--with` | What starts | How the key arrives |
| --- | --- | --- |
| `claude` | Claude Code on the Anthropic-compatible endpoint, in a home of its own | `ANTHROPIC_AUTH_TOKEN` |
| `codex` | Codex on the OpenAI-compatible endpoint, in a home of its own | `VVAULT_API_KEY`, read by a provider defined on the command line |
| `shell` | The provider's tool command, or your shell | The provider's variable (for example `DEEPSEEK_API_KEY`), plus `OPENAI_*` and `ANTHROPIC_*` for the endpoints it has |

Codex needs an endpoint that speaks the Responses API; not every provider does.

## Shared instructions

Point Versatile Vault at one folder and every managed account links to it: `CLAUDE.md`, `skills`, `commands`, `agents`, `rules` for Claude Code; `AGENTS.md`, `skills`, `prompts` for Codex. `settings.json` / `config.toml` are copied once into new profiles, with credential settings stripped.

```sh
vvault kit claude --dir ~/.claude
vvault kit codex --dir ~/.codex
```

Files you already have inside a profile are never replaced.

## Limits, plan and balance

| Account | Source | Freshness |
| --- | --- | --- |
| Claude Code sign-in | Anthropic's account endpoints, plus the `rate_limits` field Claude Code sends to its status line during a session | Right after sign-in, then at most once a minute |
| Codex sign-in | The Codex CLI's own `account/read` and `account/rateLimits/read` requests | Right after sign-in, then at most once a minute |
| Grok sign-in | Session state only; the Grok CLI does not report limits | — |
| OpenRouter and DeepSeek keys | The provider's key or balance request | At most every two minutes |
| Other API keys | None | — |

On macOS Claude Code keeps its sign-in in the Keychain rather than on disk, so Claude limits there come only from the status line.

Versatile Vault adds its status line through `--settings` at launch, so your `settings.json` is not modified. If you already have a status line, it keeps running and its output is what you see.

## Models

The model field is a list taken from the account itself, with "Other…" for anything not listed.

| Account | Where the list comes from |
| --- | --- |
| Claude Code sign-in | The aliases Claude Code documents (`fable`, `opus`, `sonnet`, `haiku`, …) plus the options Claude Code cached for that account |
| Codex sign-in | The Codex CLI's `model/list` request |
| Grok | `grok models` |
| API key or provider | The endpoint's models listing |

```sh
vvault models work
vvault models --harness codex
```

## Keeping sessions alive

The tray app renews idle accounts in the background by asking each harness for its own status. Without the app, schedule the same thing:

```sh
vvault keepalive        # exit code 1 when an account needs a new sign-in
```

## Bars and widgets

```sh
vvault list --json
vvault waybar           # {"text": "37%", "tooltip": "…", "class": "ok|warning|critical"}
```
