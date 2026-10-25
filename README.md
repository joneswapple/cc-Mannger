# cliproxy

A small Go server that lets a LAN of terminals share **one** logged-in Claude
Code CLI session. The operator runs `claude /login` once on the host machine;
cliproxy reads that machine's OAuth tokens, refreshes them on demand, and
forwards Anthropic-compatible requests from per-terminal API keys it issues
itself.

中文文档：[README.zh.md](./README.zh.md)

## What you get

- `POST /v1/messages` — Anthropic-compatible Messages API endpoint with full
  SSE streaming pass-through. Set `ANTHROPIC_BASE_URL` + `ANTHROPIC_API_KEY`
  on a client and unmodified `claude` works.
- Invite-token self-service registration so each terminal gets its own
  `sk-cp-…` key (bcrypt-hashed at rest, single shown-once display).
- Per-user policies: enable/disable, model allowlist, RPM limit, daily token
  quota. A global model denylist applies on top.
- Usage stats per request (tokens, latency, status) with by-day, by-user,
  by-model rollups.
- A minimal HTML dashboard under `/admin/` for managing invites, users, keys,
  policies, and viewing stats.
- Single static binary, no CGO, embedded SQLite store and assets.

## Quickstart

```sh
# 1. Build
go build -o cliproxy ./cmd/cliproxy

# 2. Make sure Claude Code is logged in on this host:
#    claude /login   (or `claude` and follow the OAuth prompt)
#    The credentials live at ~/.claude/.credentials.json by default.

# 3. Copy the sample config and edit if needed.
cp config.example.yaml config.yaml

# 4. Start the proxy. On first run an admin token is printed to stdout —
#    save it now, you cannot recover it later.
./cliproxy serve --config ./config.yaml

# 5. Sign in to http://<host>:8787/admin/ with the printed token, or use
#    the admin API to create an invite:
curl -X POST http://127.0.0.1:8787/admin/api/invites \
     -H "Authorization: Bearer <admin-token>" \
     -H "Content-Type: application/json" \
     -d '{"label":"alice-laptop","ttl_hours":24}'

# 6. From a LAN client, redeem the invite for an API key:
curl -X POST http://<host>:8787/register \
     -H "Content-Type: application/json" \
     -d '{"token":"<invite-token>","label":"alice"}'

# 7. Point Claude Code at the proxy:
export ANTHROPIC_BASE_URL=http://<host>:8787
export ANTHROPIC_API_KEY=sk-cp-...
claude
```

## Configuration

A fully-commented sample lives at [`config.example.yaml`](./config.example.yaml).
Copy it to `config.yaml` (or any path you pass to `--config`) and edit. Order
of precedence: code defaults → YAML file → `CLIPROXY_*` env vars.

| YAML key                       | Env override                  | Default                                                  |
| ------------------------------ | ----------------------------- | -------------------------------------------------------- |
| `listen`                       | `CLIPROXY_LISTEN`             | `0.0.0.0:8787`                                           |
| `data_dir`                     | `CLIPROXY_DATA_DIR`           | `~/.local/share/cliproxy`                                |
| `log_level`                    | `CLIPROXY_LOG_LEVEL`          | `info`                                                   |
| `upstream.base_url`            | `CLIPROXY_UPSTREAM_BASE_URL`  | `https://api.anthropic.com`                              |
| `upstream.credentials_path`    | `CLIPROXY_CREDENTIALS_PATH`   | auto-detect (see below)                                  |
| `upstream.oauth_token_url`     | —                             | `https://console.anthropic.com/v1/oauth/token`           |
| `upstream.oauth_client_id`     | —                             | `9d1c250a-e61b-44d9-88ed-5944d1962f5e`                   |
| `upstream.anthropic_beta`      | —                             | `oauth-2025-04-20`                                       |
| `upstream.user_agent`          | —                             | `claude-cli/1.0 (cliproxy)`                              |
| `upstream.sync_credentials_file` | `CLIPROXY_SYNC_CREDENTIALS_FILE` | `true`                                              |
| `secret_key`                   | `CLIPROXY_SECRET_KEY`         | empty (use / create `data_dir/secret.key`)               |
| `admin.initial_token`          | `CLIPROXY_ADMIN_TOKEN`        | empty (generate random on first run)                     |

Upstream credentials are stored as **accounts** in the database, with tokens
encrypted (AES-256-GCM) using `secret_key` or `data_dir/secret.key`. Back up
that key file together with `cliproxy.db`. On first start, when there are no
accounts yet, the Claude Code credential file is imported as account #1; with
`sync_credentials_file` on (the default) the file stays authoritative, so the
host's own `claude` keeps working alongside cliproxy. The file is found in
this order:

1. `upstream.credentials_path` from config / `CLIPROXY_CREDENTIALS_PATH`
2. `$CLAUDE_CREDENTIALS_PATH`
3. `$CLAUDE_CONFIG_DIR/.credentials.json`
4. `$HOME/.claude/.credentials.json`

On macOS Claude Code stores OAuth tokens in the keychain rather than a file.
Until cliproxy adds direct keychain support, export them once with
`security find-generic-password -s "Claude Code-credentials" -w` and point
`upstream.credentials_path` at the resulting file.

## HTTP API

| Method | Path                                       | Auth                | Description                                          |
| ------ | ------------------------------------------ | ------------------- | ---------------------------------------------------- |
| `POST` | `/v1/messages`                             | `x-api-key`         | Anthropic-compatible Messages endpoint (SSE + JSON). |
| `POST` | `/v1/messages/count_tokens`                | `x-api-key`         | Token-counting passthrough; same headers, no stats.  |
| `GET`  | `/v1/models`                               | `x-api-key`         | Lists models from the cached upstream catalog.       |
| `POST` | `/register`                                | invite token (body) | Redeem an invite for a fresh `sk-cp-…` key.          |
| `GET`  | `/healthz`                                 | —                   | Liveness probe.                                      |
| `GET`  | `/admin/login`                             | —                   | Login form.                                          |
| `POST` | `/admin/login`                             | form (`token`)      | Sets the `cliproxy_admin` cookie.                    |
| `POST` | `/admin/logout`                            | cookie              | Clears the cookie.                                   |
| `GET`  | `/admin/users` `/invites` `/stats` `/settings` | cookie/bearer    | Server-rendered dashboard.                           |
| `POST` | `/admin/api/invites`                       | cookie/bearer       | Create an invite (`{label, ttl_hours}`).             |
| `POST` | `/admin/api/invites/{hash}/delete`         | cookie/bearer       | Delete an unredeemed invite.                         |
| `POST` | `/admin/api/users/{id}/{enable,disable}`   | cookie/bearer       | Toggle a user.                                       |
| `POST` | `/admin/api/users/{id}/policy`             | cookie/bearer       | Set per-user policy.                                 |
| `POST` | `/admin/api/keys/issue`                    | cookie/bearer       | Issue a new key for an existing user.                |
| `POST` | `/admin/api/keys/{id}/revoke`              | cookie/bearer       | Revoke a key.                                        |
| `POST` | `/admin/api/settings/disabled-models`      | cookie/bearer       | Set the global model denylist (JSON `{"models":[...]}`). |
| `POST` | `/admin/api/settings/refresh-models`       | cookie/bearer       | Re-fetch the model catalog from Anthropic immediately. |
| `GET`  | `/admin/api/stats?days=N`                  | cookie/bearer       | Usage rollups for the last `N` days.                 |
| `GET`  | `/admin/api/usage`                          | cookie/bearer       | Searchable usage events. Query params: `q`, `user_id`, `model`, `status` (`ok`\|`error`\|`4xx`\|`5xx`\|literal code), `since`/`until` (`YYYY-MM-DDTHH:MM`), `min_tokens`, `only_error`, `sort` (`time_desc`\|`time_asc`\|`tokens_desc`\|`tokens_asc`\|`latency_desc`\|`latency_asc`), `page`, `page_size`. |

Errors from `/v1/messages` are returned in Anthropic's
`{"type":"error","error":{"type":"...","message":"..."}}` shape so unmodified
`claude` clients surface them sensibly.

## Effort dry-run CLI

`cliproxy effort` lets you inspect or preview the effort-tier rewrite
without standing the server up. The same `applyEffort` the proxy uses at
runtime is invoked locally against the configured `effort_levels`.

```sh
# What tiers does this config define?
cliproxy effort list

# Preview a single tier's expansion.
cliproxy effort expand --tier high

# Preview the full rewrite of a request body.
cliproxy effort expand --body '{"model":"claude-opus-4-7","effort":"max","messages":[]}'

# Pipe a body in.
echo '{"model":"x","effort":"medium"}' | cliproxy effort expand
```

## Admin CLI

Most admin subcommands shell over HTTP to the running server. Set
`CLIPROXY_ADMIN_TOKEN` (and optionally `CLIPROXY_ADMIN_URL` if the proxy is
not on `127.0.0.1:8787`):

```sh
cliproxy admin invite create --label alice --ttl-hours 24
cliproxy admin user disable 3
cliproxy admin user enable 3
cliproxy admin key revoke 7
```

The one exception is **`cliproxy admin reset-token`**, which writes directly
to the local SQLite database. It does not need (and should not be run while)
the server is up.

## Recovering a lost admin token

The bootstrap admin token is shown exactly once when cliproxy first starts.
If you lose it, the dashboard will redirect every `/admin/*` request back to
`/admin/login?next=…` and there's no way to authenticate. To recover:

```sh
# Stop the running server first.
./cliproxy --config ./config.yaml admin reset-token
# Or set the new token explicitly:
./cliproxy --config ./config.yaml admin reset-token --token my-new-secret
# Restart:
./cliproxy --config ./config.yaml serve
```

You can also pin the admin token in `config.yaml` under `admin.initial_token`
(or via `CLIPROXY_ADMIN_TOKEN` env var) — when set, cliproxy installs that
exact value on every startup, so as long as you keep the config file you can
never get locked out.

## Operating notes

- **Using cliproxy as `claude`'s backend**. cliproxy is shaped so the
  unmodified Claude Code CLI can point at it
  (`ANTHROPIC_BASE_URL=http://proxy.lan:8787`). Specifically:
  - `GET /v1/models` is answered from cliproxy's own catalog cache, so
    `claude`'s startup discovery doesn't need to reach
    `api.anthropic.com` directly.
  - The inbound `anthropic-beta` header is **merged** with the configured
    defaults (deduplicated, comma-separated), so `claude`-specific feature
    flags like `interleaved-thinking-…` and
    `fine-grained-tool-streaming-…` reach the upstream alongside the
    OAuth-required `claude-code-…,oauth-…` pair.
  - The inbound `User-Agent` is forwarded only when it begins with
    `claude-cli/`, `claude-code/`, `anthropic-`, or `claude/`. Other UAs
    (a plain `curl/8.x`, third-party SDKs) are replaced with the
    configured default — this avoids tripping Anthropic's anti-abuse
    layer while keeping legitimate Claude Code traffic identifiable.

- **OpenAI-style effort tier**. `/v1/messages` accepts an extra
  top-level `"effort"` field that aliases an Anthropic `max_tokens` +
  extended-thinking `budget_tokens` pair. Built-in tiers (configurable
  under `upstream.effort_levels`):

  | tier   | max_tokens | thinking budget |
  | ------ | ---------- | --------------- |
  | auto   | 8192       | —               |
  | low    | 4096       | —               |
  | medium | 16384      | 5000            |
  | high   | 32768      | 15000           |
  | xhigh  | 48000      | 30000           |
  | max    | 64000      | 60000           |

  cliproxy strips the `effort` field before forwarding (Anthropic doesn't
  recognise it). An explicit `max_tokens` or `thinking` already in the
  request is always preserved — `effort` only fills in defaults. An
  unknown tier returns 400 with the list of valid values.

- **System-prompt injection**. Anthropic's OAuth Pro/Max endpoint requires
  the literal `You are Claude Code, Anthropic's official CLI for Claude.`
  as the first `system` block of every `/v1/messages` request. If a
  request arrives without it (e.g. a plain `curl` or a third-party SDK
  that doesn't know to prepend it), Anthropic returns the *exact same*
  terse `rate_limit_error: "Error"` payload it returns for real throttling.
  By default cliproxy rewrites the request body to prepend that block
  before forwarding, leaving any pre-existing system content in place
  after it. Requests that already start with the identifier (e.g. coming
  from `claude` itself) pass through byte-for-byte. Set
  `upstream.inject_claude_code_system: false` to opt out.
- **Upstream errors**. The proxy forwards Anthropic's response body and
  status code byte-for-byte (so unmodified `claude` clients get the
  expected `{"type":"error","error":{...},"request_id":"..."}` shape and
  `retry-after` / `anthropic-ratelimit-*` headers). On any 4xx/5xx the
  server log gets a structured line with the request_id, error type,
  message, status, retry-after, and which user hit it; the same summary is
  saved into `usage_events.error`. Anthropic's OAuth-bearer rate limits
  routinely come back with a terse `"message":"Error"` — that's their
  shape, not ours, and the request_id is the only thing upstream support
  can cross-reference.
- **Model catalog**. On startup (and every 30 minutes thereafter) cliproxy
  calls `GET /v1/models` against Anthropic with the host's OAuth token to
  discover which models the current Pro/Max plan exposes. The dashboard's
  Settings page renders that list as checkboxes — tick any model to add it
  to the global denylist. If a previously denied model disappears from the
  upstream catalog (Anthropic dropped it from your plan, you renamed it,
  etc.) the dashboard surfaces it under "Previously selected, no longer
  available" so it doesn't silently rot. Hit "Refresh catalog" or
  `POST /admin/api/settings/refresh-models` to refetch on demand.
- **Process lifetime**. Run cliproxy under systemd, supervisord, or your
  preferred process manager. Send `SIGTERM` for graceful shutdown.
- **OAuth refresh**. Tokens are refreshed in-process when within 60s of
  expiry. The refreshed access/refresh token pair is written back atomically
  to the same credentials file Claude Code uses, so the host CLI stays
  logged in.
- **Backups**. The SQLite database under `data_dir` holds users, hashed keys,
  policies, and usage history. Back it up like any small SQLite DB. WAL mode
  is enabled, so prefer `sqlite3 cliproxy.db ".backup '/path/to/copy.db'"`
  over a raw file copy.
- **TLS**. cliproxy speaks plain HTTP. On any real network, terminate TLS in
  Caddy or nginx and bind cliproxy to `127.0.0.1`.
- **Resetting state**. Stop the proxy and delete `data_dir/cliproxy.db` to
  wipe everything. The next run will re-bootstrap.

## Layout

```
cmd/cliproxy/         cobra entry point
config.example.yaml   sample configuration (copy to config.yaml and edit)
internal/config       yaml + env config loader
internal/store        embedded-SQLite store and migrations
internal/auth         api keys, invites, admin token
internal/upstream     CredentialProvider + Claude CLI OAuth implementation
internal/policy       allowlist + RPM + daily-quota gate
internal/stats        async usage recorder + rollup queries
internal/server       HTTP routes, proxy handler, dashboard, embedded assets
```

## Caveats

- **Anthropic ToS**: sharing a Pro/Max session across multiple users may
  violate the terms of your plan. Make sure you understand the policy before
  running this.
- **No TLS** out of the box. Run behind Caddy/nginx on a real network.
- The OAuth refresh endpoint, client ID, and `anthropic-beta` header value
  used to talk to Anthropic over a Pro/Max OAuth token are not officially
  documented; they're configurable here so you can update them if Anthropic
  changes the contract.
