# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

CC Manager lets a LAN of terminals share logged-in Claude Pro/Max OAuth
accounts. It holds the accounts' tokens (encrypted), refreshes them,
schedules requests across them with failover, and forwards Anthropic Messages
and OpenAI Chat Completions requests from per-terminal `sk-cp-…` keys it
issues itself. It is a Tauri v2 desktop app plus a headless server over one
Rust core, modelled on lbjlaq/Antigravity-Manager. Claude OAuth accounts
only — no Anthropic API-key accounts. Phase plan: `docs/ROADMAP.zh.md`
(R0–R6 are done; the Go cliproxy has been removed).

## Build / run / test

- `cargo test --workspace` — all Rust tests (`crates/ccm-core/tests/proxy.rs`
  runs the HTTP layer end to end against a fake upstream).
- `cargo test -p ccm-core --test proxy streams_sse` — a single test.
- `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo fmt --all --check` — must stay clean.
- `pnpm install && pnpm build` (`pnpm typecheck` for types only) — the UI.
- `cargo run -p ccm-server -- serve` — headless server; prints the admin
  token on first start. `pnpm tauri dev` — desktop app.
- Building `src-tauri` on Linux needs `libwebkit2gtk-4.1-dev`,
  `libayatana-appindicator3-dev`, `librsvg2-dev`.

## Architecture

- `crates/ccm-core` — all logic, no GUI dependencies. `Core` (lib.rs) wires
  every long-lived component once; it is cheap to clone and is the Axum state.
  - `config.rs` — defaults → `config.json` → `CCM_*` env.
  - `store.rs` — one rusqlite connection behind a mutex; migrations in
    `migrations/*.sql` are listed in `MIGRATIONS` and recorded in
    `schema_migrations`, so a new migration may be non-idempotent. Never hold
    `store.conn()` across an `.await`.
  - `secret.rs` — AES-256-GCM; AAD binds ciphertext to `account:{id}:{field}`.
  - `accounts.rs` — upstream OAuth accounts; tokens only in `account_secrets`.
  - `oauth.rs` — Claude Code credentials file shapes, atomic rewrite, refresh.
  - `provider.rs` — `AccountProvider` (per-account token + refresh, optional
    two-way sync with a credentials file, `needs_reauth` on invalid_grant),
    first-start import.
  - `pool.rs` — `AccountPool`: eligibility (enabled, group, active, not
    cooling), quota soft limit, sticky sessions, strategies (priority /
    smooth weighted round-robin / least-used), `cool_down` persisted in
    `cooldown_until`, `preview()` without side effects.
  - `login.rs` — OAuth PKCE login (pending states in memory, 15 min TTL)
    and profile lookup; `onboard.rs` — upsert-by-email, imports, refresh.
  - `quota.rs` — 5h/7d usage from `anthropic-ratelimit-unified-*` headers
    or the optional usage endpoint, stored in `account_quota`.
  - `events.rs` — broadcast of needs_reauth / cooldown / all-cooling /
    quota updates (sent by `AccountStore` and the pool).
  - `routing.rs` — model routes (exact or `re:` regex) in
    `global_settings`, optional background downgrade.
  - `openai.rs` — Chat Completions ⇄ Messages translation incl. SSE.
  - `metrics.rs` — Prometheus text from `Recorder` counters and accounts.
  - `auth/` — `sk-cp-` keys (bcrypt, 12-char prefix index, Go 8-char prefixes
    still resolve, 60 s cache keyed by sha256), invites (sha256 only),
    admin token (bcrypt in `global_settings`).
  - `policy.rs` — global denylist, allowlist, RPM bucket, daily quota.
  - `stats.rs` — `Recorder` writes on its own thread via a bounded channel
    (`flush()` for tests); rollups and the usage search.
  - `upstream.rs` — `Upstream::forward` is the failover loop: pick, send,
    record quota headers, `classify` the status (429/403 cool down, 401
    forces refresh, 5xx/529 retry, else done), never reading bodies so SSE
    stays untouched. Injects Bearer, `anthropic-version`, merged
    `anthropic-beta`, User-Agent; per-account proxy clients (no direct
    fallback). `Catalog` caches `/v1/models`.
  - `rewrite.rs` — effort-tier expansion and Claude Code system injection
    (key order preserved via serde_json `preserve_order`).
  - `capture.rs` — usage / error extraction; `TeeStream` passes SSE bytes
    through and reports once on end, error or client disconnect.
  - `http/` — `mod.rs` router, `ApiIdentity` / `AdminAuth` extractors,
    error helpers, `call_local` + `TrustedLocal` (in-process admin bridge);
    `proxy.rs` the `/v1` handlers on one `dispatch` pipeline (routing →
    effort → policy → injection → forward → stats); `admin.rs`
    `/admin/api/*`, `/register`, `/oauth/callback`.
- `crates/ccm-server` — headless binary: `serve`, `config`, `admin …`
  (HTTP client over `/admin/api/*`, except `reset-token`), `effort …`,
  `migrate-from-go`. `build.rs` + rust-embed embed `dist/` (placeholder page
  when the UI isn't built) and serve it at `/`.
- `src-tauri` — desktop shell: runs `ccm_core::http::serve` in-process
  (start/stop from the tray), tray with current account / quota / switch,
  notifications from `CoreEvent`s, hide-to-tray. Commands: `admin_request`
  (the UI bridge) and `open_url`. NSIS hooks in `src-tauri/windows/`.
- `src/` + `index.html` — React + TypeScript + Vite + Tailwind UI for both
  shells; `src/api.ts` sends every call to `/admin/api/*`, via the
  `admin_request` bridge in the desktop app or HTTP + admin token in a
  browser. Pages in `src/pages/`, shared widgets in `src/ui.tsx`.
- Packaging: `Dockerfile` / `docker-compose.yml`, `scripts/install.{sh,ps1}`,
  `.github/workflows/{ci,release}.yml`.

## Conventions worth knowing

- **Anthropic error shape.** Everything under `/v1/*` (including auth
  rejections) returns `{"type":"error","error":{type,message}}` via
  `http::anthropic_error`. Admin endpoints use `http::json_error`.
- **Single-shown secrets.** Invite tokens, new API keys and generated admin
  tokens are returned exactly once. Never log them or upstream tokens.
  Admin responses never include token material (account list is metadata).
- **bcrypt off the async workers.** Key lookup, invite redeem, key issue and
  admin verify run in `spawn_blocking`.
- **New admin actions** need an `/admin/api/*` endpoint and a
  `ccm-server admin …` subcommand. The desktop app reaches every admin
  endpoint through the `admin_request` bridge, so don't add per-endpoint
  Tauri commands; the UI calls it via `src/api.ts`.
- **Upstream wire format** (headers, OAuth) changes go in `upstream.rs` /
  `oauth.rs`, with a fake-server test.
- **Config precedence:** defaults → `config.json` → `CCM_*` env; add new
  options to `config.rs` (`Default` impl + `apply_env` when overridable).

## Working in this repo

- Development branch for the current task: `claude/epic-gauss-en0jx3`.
  `main` is the upstream default.
