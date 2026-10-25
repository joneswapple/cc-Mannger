# CC Manager

A desktop app (Tauri) and headless server that let a LAN of terminals share
your logged-in **Claude Pro/Max** accounts. CC Manager keeps the accounts'
OAuth tokens (encrypted), refreshes them, spreads requests across them with
failover, and forwards Anthropic Messages API — and OpenAI Chat Completions —
requests from per-terminal API keys it issues itself. Modelled on
[lbjlaq/Antigravity-Manager](https://github.com/lbjlaq/Antigravity-Manager),
for Claude only.

中文文档：[README.zh.md](./README.zh.md) · Roadmap: [docs/ROADMAP.zh.md](./docs/ROADMAP.zh.md)

> **Scope.** Personal / testing use with your own Claude OAuth accounts.
> Anthropic API-key accounts are intentionally not supported. Anthropic's
> consumer terms do not allow sharing or reselling Pro/Max subscriptions.

## What you get

- **Proxy**: `POST /v1/messages` (Anthropic, SSE pass-through) and
  `POST /v1/chat/completions` (OpenAI-compatible, streaming, tools, images).
  Point `ANTHROPIC_BASE_URL` + `ANTHROPIC_API_KEY` at it and an unmodified
  `claude` works.
- **Accounts**: OAuth login (PKCE), import of Claude Code credentials (file,
  batch JSON, macOS keychain), the host's login imported automatically,
  tokens AES-256-GCM encrypted, optional per-account HTTP/SOCKS5 proxy.
- **Scheduling**: priority / weighted round-robin / least-used, sticky
  sessions that keep prompt caches warm, automatic failover on
  429 / 401 / 403 / 5xx with cooldowns, account groups, and quota awareness
  (5-hour / 7-day usage read from Anthropic's rate-limit headers).
- **Terminals**: invite-based self-registration, `sk-cp-…` keys (bcrypt,
  shown once), per-terminal policies (model allowlist, RPM, daily tokens,
  account group), a global model denylist and model routes (aliases/regex).
- **Admin**: React web UI (desktop window or browser), JSON API, CLI, usage
  stats and search, Prometheus `/metrics`.
- **Desktop**: tray with current account and quota, account switching,
  start/stop, notifications when an account needs a new login or everything
  is rate limited. Installers: NSIS (Windows), AppImage/deb, dmg.
- **Headless**: `ccm-server` single binary with the UI embedded, Docker image,
  install scripts.

## Quickstart

### Desktop

Download the installer from the [releases](https://github.com/joneswapple/cc-Mannger/releases)
(or build it, see Development). On first start it imports the Claude Code
login found on the machine and starts the proxy on port 8787. Add more
accounts under **上游账号 → OAuth 登录**, create a terminal under **终端**, and
copy the snippets from **接入**.

### Headless

```sh
# Install (Linux/macOS; Windows: scripts/install.ps1) or `cargo build --release -p ccm-server`
curl -fsSL https://raw.githubusercontent.com/joneswapple/cc-Mannger/main/scripts/install.sh | sh

ccm-server serve                     # prints the admin token on first start
# open http://<host>:8787/ and sign in with the admin token
```

Or with Docker: `docker compose up -d` (see [docker-compose.yml](./docker-compose.yml)),
then `docker compose logs` for the admin token.

### Connect a terminal

```sh
# Redeem an invite (created in the UI or with `ccm-server admin invite create`)
curl -X POST http://<host>:8787/register -H "Content-Type: application/json" \
     -d '{"token":"<invite-token>","label":"alice"}'

export ANTHROPIC_BASE_URL=http://<host>:8787
export ANTHROPIC_API_KEY=sk-cp-...
claude
```

OpenAI clients use `base_url=http://<host>:8787/v1` with the same key.

## Configuration

Precedence: code defaults → `config.json` → `CCM_*` environment variables.
A full sample is [`config.example.json`](./config.example.json). The desktop
app reads `<app config dir>/config.json`; `ccm-server` takes `--config` /
`CCM_CONFIG`.

| JSON key                             | Env override                  | Default                                        |
| ------------------------------------ | ----------------------------- | ---------------------------------------------- |
| `listen`                             | `CCM_LISTEN`                  | `0.0.0.0:8787`                                 |
| `data_dir`                           | `CCM_DATA_DIR`                | `<platform data dir>/cc-manager`               |
| `log_level`                          | `CCM_LOG_LEVEL` / `RUST_LOG`  | `info`                                         |
| `secret_key`                         | `CCM_SECRET_KEY`              | empty → `data_dir/secret.key` (auto-created)   |
| `admin.initial_token`                | `CCM_ADMIN_TOKEN`             | empty → random token printed on first start    |
| `upstream.base_url`                  | `CCM_UPSTREAM_BASE_URL`       | `https://api.anthropic.com`                    |
| `upstream.credentials_path`          | `CCM_CREDENTIALS_PATH`        | auto-detect (see below)                        |
| `upstream.sync_credentials_file`     | `CCM_SYNC_CREDENTIALS_FILE`   | `true`                                         |
| `upstream.oauth_token_url`           | —                             | `https://console.anthropic.com/v1/oauth/token` |
| `upstream.oauth_client_id`           | —                             | `9d1c250a-e61b-44d9-88ed-5944d1962f5e`         |
| `upstream.oauth_authorize_url`       | —                             | `https://claude.ai/oauth/authorize`            |
| `upstream.oauth_redirect_uri`        | —                             | `https://console.anthropic.com/oauth/code/callback` |
| `upstream.oauth_scopes`              | —                             | `org:create_api_key user:profile user:inference` |
| `upstream.profile_url`               | —                             | `https://api.anthropic.com/api/oauth/profile`  |
| `upstream.usage_url`                 | —                             | empty (no polling; headers are still read)     |
| `upstream.usage_poll_secs`           | —                             | `600`                                          |
| `upstream.anthropic_beta`            | —                             | `claude-code-20250219,oauth-2025-04-20`        |
| `upstream.user_agent`                | —                             | `claude-cli/1.0 (cc-manager)`                  |
| `upstream.inject_claude_code_system` | —                             | `true`                                         |
| `upstream.effort_levels`             | —                             | see "Operating notes"                          |
| `pool.strategy`                      | `CCM_POOL_STRATEGY`           | `priority` (`round_robin`, `least_used`)       |
| `pool.sticky`                        | `CCM_POOL_STICKY`             | `true`                                         |
| `pool.sticky_ttl_secs`               | —                             | `3600`                                         |
| `pool.max_retries`                   | `CCM_POOL_MAX_RETRIES`        | `2`                                            |
| `pool.cooldown_secs`                 | —                             | `60`                                           |
| `pool.quota_soft_limit`              | —                             | `0.95`                                         |
| `routing.background_model`           | —                             | empty (off)                                    |
| `routing.background_max_tokens`      | —                             | `512`                                          |

None of the OAuth / profile / usage endpoints are publicly documented; they
are configurable so they can be adjusted if Anthropic changes them.

**Secret key.** Upstream tokens are encrypted with `secret_key` or
`data_dir/secret.key`. Back that key up together with `cc-manager.db`.

**Host login.** When no account exists yet, the Claude Code credentials file
is imported as account #1: `upstream.credentials_path`,
`$CLAUDE_CREDENTIALS_PATH`, `$CLAUDE_CONFIG_DIR/.credentials.json`, then
`~/.claude/.credentials.json`. With `sync_credentials_file` on, that file
stays authoritative so the host's own `claude` keeps working. On macOS,
import from the keychain with `ccm-server admin account import --from-keychain`.

## Accounts

- **OAuth login**: the UI (or `ccm-server admin account login`) opens
  Claude's authorize page; after you approve, the page shows `code#state` —
  paste it back. If you point `upstream.oauth_redirect_uri` at
  `http://<host>:8787/oauth/callback` (and the client id accepts it), the
  login completes automatically.
- **Import**: a `.credentials.json` document in any common shape, or a JSON
  array of them (items may be `{"label","account_group","credentials"}`).
  An account with the same email is updated rather than duplicated, which is
  also how a re-login clears `needs_reauth`.
- **Per account**: label, priority, weight, group, proxy, enable/disable,
  pin (move to top priority), refresh now, clear cooldown, delete. An
  account with a proxy sends everything through it and never falls back to
  a direct connection.

## Scheduling and failover

For each request CC Manager keeps accounts that are enabled, in the user's
group (`account_group` in the policy; unset = all), `active` and not cooling
down; prefers those under `pool.quota_soft_limit`; reuses the session's
sticky account (Claude Code's `metadata.user_id`, else the terminal key);
otherwise applies `pool.strategy` — `priority`, `round_robin` (smooth,
weighted) or `least_used` (fewest tokens in 5 hours).

Before any byte reaches the client, failed attempts move to the next account
(up to `pool.max_retries` more):

| Upstream result                         | Action                                                       |
| --------------------------------------- | ------------------------------------------------------------ |
| 429                                     | cool down until `retry-after` / `anthropic-ratelimit-*-reset` (else `pool.cooldown_secs`) |
| 401                                     | force a token refresh (no refresh token → `needs_reauth`)    |
| 403                                     | cool down 30 minutes                                         |
| 500 / 502 / 503 / 504 / 529, network     | try the next account, no cooldown                            |
| anything else                           | forward as is                                                |

When accounts run out, the last upstream response is forwarded verbatim. If
every eligible account is already cooling, the proxy answers 429 with a
`retry-after` without calling upstream. Cooldowns survive restarts.

**Quota.** Every upstream response's `anthropic-ratelimit-unified-5h/7d-*`
headers update the account's usage snapshot (shown in the UI, tray and
`/metrics`). Set `upstream.usage_url` to also poll a usage endpoint.

## OpenAI compatibility

`POST /v1/chat/completions` translates to Messages and back: system /
developer messages, text and image parts (data or http URLs), tools and
`tool_choice`, assistant `tool_calls` and `tool` results, `stop`,
`max_completion_tokens`, `reasoning_effort` (mapped to the effort tiers),
`stream` with `stream_options.include_usage`. Responses carry `tool_calls`,
`reasoning_content` and usage. Use model routes to map OpenAI model names,
e.g. `re:^gpt-4` → `claude-sonnet-4-5`. `/v1/models` includes both Anthropic
and OpenAI fields.

## HTTP API

Admin endpoints need `Authorization: Bearer <admin token>` (or
`x-admin-token`, or the `ccm_admin` cookie); timestamps are unix seconds.

| Method | Path                                         | Auth          | Description                                             |
| ------ | -------------------------------------------- | ------------- | ------------------------------------------------------- |
| `POST` | `/v1/messages`                               | API key       | Anthropic Messages (SSE + JSON).                        |
| `POST` | `/v1/chat/completions`                       | API key       | OpenAI Chat Completions.                                |
| `POST` | `/v1/messages/count_tokens`                  | API key       | Token counting; no policy, no stats.                    |
| `GET`  | `/v1/models`                                 | API key       | Cached upstream model catalog.                          |
| `POST` | `/register`                                  | invite        | `{token, label}` → `{api_key, user_id}`.                |
| `GET`  | `/oauth/callback`                            | login state   | OAuth redirect target.                                  |
| `GET`  | `/healthz`                                   | —             | `{status, version}`.                                    |
| `GET`  | `/metrics`                                   | admin         | Prometheus metrics.                                     |
| `GET`  | `/`                                          | —             | Web UI (ccm-server).                                    |
| `GET`  | `/admin/api/status`                          | admin         | Version, accounts, upstream, catalog.                   |
| `GET`  | `/admin/api/accounts`                        | admin         | Accounts with quota (never tokens), pool config, recommended account. |
| `POST` | `/admin/api/accounts/oauth/start`            | admin         | `{label?, account_group?}` → `{url, state}`.            |
| `POST` | `/admin/api/accounts/oauth/complete`         | admin         | `{code, state?}` → account.                             |
| `POST` | `/admin/api/accounts/import`                 | admin         | `{credentials, label?, account_group?}` → `{imported, errors}`. |
| `POST` | `/admin/api/accounts/{id}`                   | admin         | Update `{label, priority, weight, enabled, account_group, proxy_url, clear_cooldown}`. |
| `POST` | `/admin/api/accounts/{id}/{refresh,pin,delete}` | admin      | Refresh token + profile / move to top / delete.         |
| `GET` `POST` | `/admin/api/users`                     | admin         | List (keys, policies) / create `{label}`.               |
| `POST` | `/admin/api/users/{id}/{enable,disable}`     | admin         | Toggle a user.                                          |
| `GET` `POST` | `/admin/api/users/{id}/policy`         | admin         | `{allowed_models, daily_token_limit, rpm_limit, account_group}`. |
| `POST` | `/admin/api/keys/issue`, `/keys/{id}/revoke` | admin         | Issue (shown once) / revoke a key.                      |
| `GET` `POST` | `/admin/api/invites`, `/invites/{hash}/delete` | admin   | Invites.                                                |
| `GET`  | `/admin/api/settings`                        | admin         | Denylist, model routes, catalog, wire settings.         |
| `POST` | `/admin/api/settings/disabled-models`        | admin         | `{models}`.                                             |
| `POST` | `/admin/api/settings/model-routes`           | admin         | `{routes: [{pattern, target}]}` (`re:` for regex).      |
| `POST` | `/admin/api/settings/refresh-models`         | admin         | Refetch the model catalog.                              |
| `GET`  | `/admin/api/stats?days=N`                    | admin         | Rollups by day, user, model, account.                   |
| `GET`  | `/admin/api/usage`                           | admin         | Search: `q`, `user_id`, `account_id`, `model`, `status`, `since`, `until`, `min_tokens`, `only_error`, `sort`, `page`, `page_size`. |

Errors under `/v1/*` use Anthropic's `{"type":"error","error":{"type","message"}}`
shape (OpenAI SDKs read `error.message` from it too); admin errors are
`{"error": "..."}`.

## CLI

```sh
ccm-server serve | config
ccm-server admin status
ccm-server admin account list
ccm-server admin account login [--label x --group g]
ccm-server admin account import --file ~/.claude/.credentials.json | --from-keychain
ccm-server admin account set 2 --priority 10 --weight 3 --group team --proxy socks5://127.0.0.1:1080 --clear-cooldown
ccm-server admin account refresh 2 | pin 2 | delete 2
ccm-server admin invite create --label alice --ttl-hours 24 | list | delete <hash>
ccm-server admin user list | create --label x | enable 3 | disable 3
ccm-server admin user policy 3 --allow claude-sonnet-4-5 --rpm 10 --daily-tokens 2000000 --group team
ccm-server admin key issue 3 | revoke 7
ccm-server admin models list | disable claude-opus-4-1 | refresh
ccm-server admin models routes 're:^gpt-4=claude-sonnet-4-5' 'gpt-3.5-turbo=claude-haiku-4-5'
ccm-server admin stats --days 7
ccm-server admin usage --status error --page-size 20
ccm-server admin reset-token [--token my-secret]      # local DB, no server needed
ccm-server effort list | expand --tier high
ccm-server migrate-from-go --go-data-dir ~/.local/share/cliproxy
```

Admin subcommands use `CCM_ADMIN_TOKEN` and `CCM_ADMIN_URL`
(default `http://127.0.0.1:8787`).

## Operating notes

- **Effort tiers.** A top-level `"effort"` fills in `max_tokens` and
  extended thinking (auto 8192 · low 4096 · medium 16384 + 5000 ·
  high 32768 + 15000 · xhigh 48000 + 30000 · max 64000 + 60000); explicit
  values win; unknown tiers get a 400.
- **System-prompt injection.** The OAuth Pro/Max endpoint requires
  `You are Claude Code, Anthropic's official CLI for Claude.` as the first
  system block and otherwise answers a misleading `rate_limit_error`. It is
  prepended when missing; disable with `upstream.inject_claude_code_system`.
- **Headers.** The inbound `anthropic-beta` is merged with the configured
  flags; the inbound `User-Agent` is forwarded only for `claude-cli/`,
  `claude-code/`, `anthropic-` and `claude/` clients.
- **Upstream errors** are forwarded byte-for-byte with status and headers; a
  summary with `request_id` is logged and stored in usage.
- **Metrics.** `/metrics` exposes request and token counters per account and
  status, account up/cooldown gauges and quota utilization. Scrape it with the
  admin token as a bearer token.
- **Backups.** Back up `data_dir` (`cc-manager.db` + `secret.key`).
- **Migrating from the Go cliproxy**: `ccm-server migrate-from-go` copies
  users, keys, invites, policies, the admin token and usage history into a
  fresh data dir, keeping ids so existing keys keep working.

## Development

```sh
pnpm install && pnpm build          # UI (embedded into ccm-server at build time)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo run -p ccm-server -- serve    # headless
pnpm tauri dev                      # desktop; `pnpm tauri build` for installers
```

Linux desktop builds need `libwebkit2gtk-4.1-dev`,
`libayatana-appindicator3-dev` and `librsvg2-dev`. Pushing a `v*` tag runs
the release workflow (installers for all platforms, `ccm-server` binaries,
`ghcr.io` image) into a draft release.
