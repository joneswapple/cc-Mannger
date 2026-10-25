# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

cliproxy is a single-binary Go server that lets a LAN of terminals share one
logged-in Claude Code CLI session. The operator runs `claude /login` once on
the host; cliproxy reads that host's OAuth credentials, refreshes them on
demand, and forwards Anthropic Messages API requests from per-terminal API
keys it issues itself. See `README.md` for the user-facing walkthrough.

## Build / run / test

- `go build ./cmd/cliproxy` — single static binary, no CGO required
  (modernc.org/sqlite is pure Go).
- `go test ./...` — runs the suite.
- `go test ./internal/server -run TestProxy` — single subtest pattern.
- `go vet ./...` — must stay clean.
- `./cliproxy serve --config path/to/config.yaml` — run the server. On first
  run an admin token is printed to stdout.
- `./cliproxy admin invite create --label foo` — admin subcommands hit the
  local server's admin API (uses `CLIPROXY_ADMIN_TOKEN` and `CLIPROXY_ADMIN_URL`).

## Architecture (the parts that span files)

- `cmd/cliproxy/main.go` is the cobra entry point. `serve` wires every
  long-lived component (`store`, `auth.Keys`, `auth.Invites`, `auth.Admin`,
  `upstream.ClaudeCLIProvider`, `upstream.Client`, `policy.Engine`,
  `stats.Recorder`, `server.Server`) and starts the HTTP listener.
- `internal/store` owns the SQLite connection. Migrations are embedded under
  `internal/store/migrations/` via `embed.FS` and applied on `Open`.
- `internal/auth`
  - `keys.go` issues `sk-cp-…` keys (bcrypt-hashed, 8-char prefix index for
    fast lookup, in-memory cache with 60s TTL).
  - `invite.go` issues single-use invite tokens (only the sha256 is stored).
  - `admin.go` bootstraps the dashboard admin token and verifies it.
- `internal/upstream`
  - `provider.go` defines `CredentialProvider`. Future Bedrock/Vertex/API-key
    providers should implement this interface.
  - `claudecli.go` reads the Claude Code CLI credentials file and refreshes
    via the OAuth token endpoint. The OAuth URL, client_id, and
    `anthropic-beta` header are config-driven because they're not officially
    documented.
  - `client.go` is the upstream HTTP client that injects `Authorization: Bearer`,
    `anthropic-version`, `anthropic-beta`, and `User-Agent` for every forward.
- `internal/policy/policy.go` is the request-time gate (model allowlist,
  global denylist, in-memory RPM bucket, DB-backed daily token quota).
- `internal/stats/recorder.go` is async — handlers push events on a channel
  and a goroutine writes them; aggregation queries are also defined here.
- `internal/server`
  - `server.go` builds the chi router and embeds `templates/*.html` and
    `static/*` so the binary is self-contained.
  - `proxy.go` handles `POST /v1/messages`. SSE responses are streamed
    line-by-line to the client while teeing through an SSE parser that
    captures token usage from `message_start` and `message_delta` events;
    non-SSE responses are buffered just enough to parse the trailing `usage`
    object.
  - `admin_api.go` exposes JSON endpoints under `/admin/api/*`. The
    `cliproxy admin …` CLI is a thin wrapper over these.
  - `dashboard.go` renders the HTML pages (server-rendered with
    `html/template`; the only JS is small inline `fetch` calls, no
    framework).
  - `middleware.go` holds `requireAPIKey` and `requireAdmin`. API errors are
    returned in Anthropic's `{type:"error",error:{type,message}}` shape so
    Claude Code surfaces them sensibly.

## Conventions worth knowing

- **No CGO.** Stick to `modernc.org/sqlite`. Don't add a dependency that
  pulls in libc.
- **Embedded assets.** All templates and CSS are embedded into the binary
  (`internal/server/templates/`, `internal/server/static/`). Add new pages
  there, not in a separate `web/` tree.
- **Anthropic error shape.** Any error returned from `/v1/messages` (or the
  middleware that protects it) must be the Anthropic-shaped JSON, not a
  generic JSON error. See `writeAnthropicError` in `internal/server/middleware.go`.
- **Single-shown secrets.** Invite tokens, fresh API keys, and the bootstrap
  admin token are returned exactly once. Never log them.
- **Config precedence:** defaults → YAML file → `CLIPROXY_*` env vars. New
  options should follow that pattern in `internal/config/config.go`.

## Working in this repo

- The default development branch for AI-driven changes in the current task
  is `claude/add-claude-documentation-rpN2g`. `main` is the upstream default.
- When changing the upstream wire format (headers, OAuth flow), update both
  `internal/upstream/claudecli.go` and `internal/upstream/client.go`, and
  add a fake-server unit test in `internal/upstream/claudecli_test.go`.
- When adding admin actions, expose them as both an `/admin/api/*` JSON
  endpoint and a `cliproxy admin …` cobra subcommand so users can drive the
  proxy without the dashboard.
