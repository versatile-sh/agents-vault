# How it stays safe

| Rule | What it means |
| --- | --- |
| One home per account | Every account runs with its own `CLAUDE_CONFIG_DIR`, `CODEX_HOME` or `GROK_HOME`. Credentials are never copied between folders, so a rotating refresh token only ever exists in one place. |
| The harness owns its tokens | Sign-in, refresh and sign-out are done by `claude`, `codex` and `grok` themselves, inside the account's home. Versatile Vault never stores, copies or refreshes an OAuth token. |
| One read-only exception | To show Claude limits and the plan name, the access token Claude Code saved is read in memory and sent only to Anthropic's own account endpoints (`api.anthropic.com/api/oauth/usage` and `/profile`). These endpoints are not documented; if they change, limits fall back to what Claude Code reports to its status line. |
| Keys live in the OS keychain | API keys go to macOS Keychain, Windows Credential Manager or the Secret Service. They are not written to the ledger, to a config file or to a command line. |
| Keys are resolved late | The terminal runs `vvault run <id>`; the key is read from the keychain there and passed to the harness through its environment only. |
| Clean environment | Variables that would override an identity (`ANTHROPIC_*`, `CODEX_HOME`, `OPENAI_API_KEY`, …) are removed before the harness starts. |
| Clear text is refused | A custom endpoint must be `https://`; plain `http://` is accepted only for localhost. |

The ledger (`ledger.json`) holds labels, models and folder paths. It is created readable only by you.
