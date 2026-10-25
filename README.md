# CC Manager

A desktop app (Tauri) and headless server that let a LAN of terminals share
your logged-in **Claude Pro/Max** accounts. You log in once on the host;
CC Manager keeps the OAuth tokens (encrypted), refreshes them on demand, and
forwards Anthropic Messages API requests from per-terminal API keys it
issues itself. Modelled on
[lbjlaq/Antigravity-Manager](https://github.com/lbjlaq/Antigravity-Manager),
for Claude only.

中文文档：[README.zh.md](./README.zh.md) · Roadmap: [docs/ROADMAP.zh.md](./docs/ROADMAP.zh.md)

> **Scope.** Personal / testing use with your own Claude OAuth accounts.
> Anthropic API-key accounts are intentionally not supported. Anthropic's
> consumer terms do not allow sharing or reselling Pro/Max subscriptions.

## What you get

- `POST /v1/messages` — Anthropic-compatible endpoint with SSE streaming
  pass-through. Point `ANTHROPIC_BASE_URL` + `ANTHROPIC_API_KEY` at it and an
  unmodified `claude` works.
- Invite-based self-service registration: each terminal redeems an invite
  for its own `sk-cp-…` key (bcrypt-hashed at rest, shown once).
- Per-user policies (enable/disable, model allowlist, RPM, daily token quota)
  plus a global model denylist.
- Usage stats per request (tokens, latency, status, upstream account) with
  daily / per-user / per-model / per-account rollups.
- Upstream accounts stored in SQLite with AES-256-GCM encrypted tokens; the
  host's Claude Code login is imported automatically on first start.
- Two shells over one Rust core: the **desktop app** (window + tray, proxy
  runs in-process) and **`ccm-server`** (headless, for servers / Docker).

## Quickstart (headless)

```sh
# 1. Build
cargo build --release -p ccm-server

# 2. Make sure Claude Code is logged in on this host (`claude /login`).
#    Credentials live at ~/.claude/.credentials.json by default.

# 3. Start. On first run an admin token is printed — save it.
./target/release/ccm-server serve        # or: --config ./config.json

# 4. Create an invite (admin CLI talks to the running server):
export CCM_ADMIN_TOKEN=<admin-token>
./target/release/ccm-server admin invite create --label alice-laptop

# 5. From a LAN client, redeem it for an API key:
curl -X POST http://<host>:8787/register \
     -H "Content-Type: application/json" \
     -d '{"token":"<invite-token>","label":"alice"}'

# 6. Point Claude Code at the proxy:
export ANTHROPIC_BASE_URL=http://<host>:8787
export ANTHROPIC_API_KEY=sk-cp-...
claude
```

## Desktop app

```sh
pnpm install
pnpm tauri dev      # development
pnpm tauri build    # installers: NSIS (Windows), AppImage/deb (Linux), dmg (macOS)
```

On Linux, building needs `libwebkit2gtk-4.1-dev`,
`libayatana-appindicator3-dev` and `librsvg2-dev`. Closing the window keeps
the proxy running in the tray. The desktop app reads
`<app config dir>/config.json` and shares the same data dir as `ccm-server`.

## Configuration

Precedence: code defaults → `config.json` → `CCM_*` environment variables.
A full sample is [`config.example.json`](./config.example.json).

| JSON key                          | Env override                     | Default                                        |
| --------------------------------- | -------------------------------- | ---------------------------------------------- |
| `listen`                          | `CCM_LISTEN`                     | `0.0.0.0:8787`                                 |
| `data_dir`                        | `CCM_DATA_DIR`                   | `<platform data dir>/cc-manager`               |
| `log_level`                       | `CCM_LOG_LEVEL` (or `RUST_LOG`)  | `info`                                         |
| `secret_key`                      | `CCM_SECRET_KEY`                 | empty → `data_dir/secret.key` (auto-created)   |
| `upstream.base_url`               | `CCM_UPSTREAM_BASE_URL`          | `https://api.anthropic.com`                    |
| `upstream.credentials_path`       | `CCM_CREDENTIALS_PATH`           | auto-detect (see below)                        |
| `upstream.sync_credentials_file`  | `CCM_SYNC_CREDENTIALS_FILE`      | `true`                                         |
| `upstream.oauth_token_url`        | —                                | `https://console.anthropic.com/v1/oauth/token` |
| `upstream.oauth_client_id`        | —                                | `9d1c250a-e61b-44d9-88ed-5944d1962f5e`         |
| `upstream.anthropic_beta`         | —                                | `claude-code-20250219,oauth-2025-04-20`        |
| `upstream.user_agent`             | —                                | `claude-cli/1.0 (cc-manager)`                  |
| `upstream.inject_claude_code_system` | —                             | `true`                                         |
| `upstream.effort_levels`          | —                                | see "Effort tiers"                             |
| `admin.initial_token`             | `CCM_ADMIN_TOKEN`                | empty → random token printed on first start    |

`ccm-server` also reads `--config` / `CCM_CONFIG` for the config path.

**Accounts and the secret key.** Upstream tokens are encrypted with
`secret_key` or `data_dir/secret.key`. Back that key up together with
`cc-manager.db` — without it stored tokens can't be decrypted. When no
account exists yet, the Claude Code credentials file is imported as account
#1, looked up in this order: `upstream.credentials_path`,
`$CLAUDE_CREDENTIALS_PATH`, `$CLAUDE_CONFIG_DIR/.credentials.json`,
`~/.claude/.credentials.json`. With `sync_credentials_file` on, that file
stays authoritative (re-read before refreshes, rewritten after), so the
host's own `claude` keeps working alongside CC Manager. On macOS, export the
keychain entry first:
`security find-generic-password -s "Claude Code-credentials" -w > ~/.claude/.credentials.json`.

## HTTP API

Timestamps in admin responses are unix seconds.

| Method | Path                                      | Auth            | Description                                           |
| ------ | ----------------------------------------- | --------------- | ----------------------------------------------------- |
| `POST` | `/v1/messages`                            | `x-api-key`     | Anthropic Messages endpoint (SSE + JSON).             |
| `POST` | `/v1/messages/count_tokens`               | `x-api-key`     | Token counting passthrough; no policy, no stats.      |
| `GET`  | `/v1/models`                              | `x-api-key`     | Models from the cached upstream catalog.              |
| `POST` | `/register`                               | invite (body)   | `{token, label}` → `{api_key, user_id}`.              |
| `GET`  | `/healthz`                                | —               | Liveness: `{status, version}`.                        |
| `GET`  | `/admin/api/status`                       | admin           | Version, accounts, upstream source, catalog state.    |
| `GET`  | `/admin/api/users`                        | admin           | Users with keys (no hashes) and policies.             |
| `POST` | `/admin/api/users`                        | admin           | Create a user (`{label}`).                            |
| `POST` | `/admin/api/users/{id}/{enable,disable}`  | admin           | Toggle a user.                                        |
| `GET` `POST` | `/admin/api/users/{id}/policy`      | admin           | Read / replace `{allowed_models, daily_token_limit, rpm_limit}`. |
| `POST` | `/admin/api/keys/issue`                   | admin           | `{user_id}` → `{api_key, key_id}`.                    |
| `POST` | `/admin/api/keys/{id}/revoke`             | admin           | Revoke a key.                                         |
| `GET` `POST` | `/admin/api/invites`                | admin           | List / create (`{label, ttl_hours}` → `{token, expires_at}`). |
| `POST` | `/admin/api/invites/{hash}/delete`        | admin           | Delete an unredeemed invite.                          |
| `GET`  | `/admin/api/accounts`                     | admin           | Upstream accounts (metadata only, never tokens).      |
| `GET`  | `/admin/api/settings`                     | admin           | Denylist, model catalog, upstream wire settings.      |
| `POST` | `/admin/api/settings/disabled-models`     | admin           | Replace the global denylist (`{models: [...]}`).      |
| `POST` | `/admin/api/settings/refresh-models`      | admin           | Refetch the model catalog now.                        |
| `GET`  | `/admin/api/stats?days=N`                 | admin           | Rollups by day, user, model, account.                 |
| `GET`  | `/admin/api/usage`                        | admin           | Search: `q`, `user_id`, `account_id`, `model`, `status` (`ok`\|`error`\|`2xx`…`5xx`\|code), `since`/`until` (unix or `YYYY-MM-DDTHH:MM` UTC), `min_tokens`, `only_error`, `sort` (`time_desc`\|`time_asc`\|`tokens_desc`\|`tokens_asc`\|`latency_desc`\|`latency_asc`), `page`, `page_size`. |

Admin auth: `Authorization: Bearer <token>`, `x-admin-token`, or the
`ccm_admin` cookie. Errors from `/v1/*` use Anthropic's
`{"type":"error","error":{"type","message"}}` shape; admin errors are
`{"error": "..."}`.

## CLI

```sh
ccm-server serve                               # default command
ccm-server config                              # effective config, secrets redacted

# Admin (needs CCM_ADMIN_TOKEN; CCM_ADMIN_URL defaults to http://127.0.0.1:8787)
ccm-server admin status
ccm-server admin invite create --label alice --ttl-hours 24
ccm-server admin invite list
ccm-server admin user list | create --label x | enable 3 | disable 3
ccm-server admin user policy 3 --allow claude-sonnet-4-5 --rpm 10 --daily-tokens 2000000
ccm-server admin key issue 3 | revoke 7
ccm-server admin account list
ccm-server admin models list | disable claude-opus-4-1 | refresh
ccm-server admin stats --days 7
ccm-server admin usage --status error --page-size 20
ccm-server admin reset-token [--token my-secret]   # writes the local DB directly

# Effort tiers
ccm-server effort list
ccm-server effort expand --tier high
echo '{"model":"x","effort":"medium"}' | ccm-server effort expand
```

**Lost the admin token?** Run `ccm-server admin reset-token` on the host (a
running server picks the new token up within a minute), or pin one with
`admin.initial_token` / `CCM_ADMIN_TOKEN`, which is installed on every start.

## Migrating from the Go cliproxy

```sh
ccm-server migrate-from-go --go-data-dir ~/.local/share/cliproxy
```

Copies users, API keys, invites, policies, the admin token, the model
denylist and usage history into a fresh CC Manager data dir, keeping ids —
existing `sk-cp-…` keys and the admin token keep working. Upstream accounts
are decrypted with the Go `secret.key` (or `--go-secret-key`) and re-sealed.

## Operating notes

- **Claude Code as client.** `/v1/models` is served from CC Manager's own
  catalog (refreshed every 30 minutes). The inbound `anthropic-beta` is
  merged with the configured flags. The inbound `User-Agent` is forwarded
  only when it starts with `claude-cli/`, `claude-code/`, `anthropic-` or
  `claude/`; others get the configured default.
- **Effort tiers.** `/v1/messages` accepts a top-level `"effort"` that fills
  in `max_tokens` and extended thinking. It is stripped before forwarding;
  explicit `max_tokens` / `thinking` always win; unknown tiers get a 400.

  | tier   | max_tokens | thinking budget |
  | ------ | ---------- | --------------- |
  | auto   | 8192       | —               |
  | low    | 4096       | —               |
  | medium | 16384      | 5000            |
  | high   | 32768      | 15000           |
  | xhigh  | 48000      | 30000           |
  | max    | 64000      | 60000           |

- **System-prompt injection.** The OAuth Pro/Max endpoint requires
  `You are Claude Code, Anthropic's official CLI for Claude.` as the first
  system block and otherwise answers with a misleading
  `rate_limit_error: "Error"`. CC Manager prepends it when missing; requests
  that already have it pass through byte-for-byte. Disable with
  `upstream.inject_claude_code_system: false`.
- **Upstream errors** are forwarded byte-for-byte with status and headers
  (`retry-after`, `anthropic-ratelimit-*`). A summary with type, message and
  `request_id` is logged and stored in `usage_events.error`.
- **OAuth refresh** happens in-process within 60 s of expiry. A rejected
  refresh token marks the account `needs_reauth`.
- **Backups.** Back up `data_dir` (`cc-manager.db` + `secret.key`).

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
pnpm install && pnpm build
```
