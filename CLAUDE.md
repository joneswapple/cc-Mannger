# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

CC Manager lets a LAN of terminals share logged-in Claude Pro/Max OAuth
accounts. It holds the accounts' tokens (encrypted), refreshes them, and
forwards Anthropic Messages API requests from per-terminal `sk-cp-…` keys it
issues itself. It is a Tauri v2 desktop app plus a headless server over one
Rust core, modelled on lbjlaq/Antigravity-Manager. Claude OAuth accounts
only — no Anthropic API-key accounts. Phase plan: `docs/ROADMAP.zh.md`
(R0 scaffold and R1 Go parity are done; the Go cliproxy has been removed).

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
    `AccountPool` (picks the account per call), first-start import.
  - `auth/` — `sk-cp-` keys (bcrypt, 12-char prefix index, Go 8-char prefixes
    still resolve, 60 s cache keyed by sha256), invites (sha256 only),
    admin token (bcrypt in `global_settings`).
  - `policy.rs` — global denylist, allowlist, RPM bucket, daily quota.
  - `stats.rs` — `Recorder` writes on its own thread via a bounded channel
    (`flush()` for tests); rollups and the usage search.
  - `upstream.rs` — reqwest client injecting Bearer, `anthropic-version`,
    merged `anthropic-beta`, User-Agent; `Catalog` caches `/v1/models`.
  - `rewrite.rs` — effort-tier expansion and Claude Code system injection
    (key order preserved via serde_json `preserve_order`).
  - `capture.rs` — usage / error extraction; `TeeStream` passes SSE bytes
    through and reports once on end, error or client disconnect.
  - `http/` — `mod.rs` router, `ApiIdentity` / `AdminAuth` extractors and
    error helpers; `proxy.rs` the `/v1` handlers; `admin.rs` `/admin/api/*`
    and `/register`.
- `crates/ccm-server` — headless binary: `serve`, `config`, `admin …`
  (HTTP client over `/admin/api/*`, except `reset-token`), `effort …`,
  `migrate-from-go`.
- `src-tauri` — desktop shell: runs `ccm_core::http::serve` in-process, tray,
  hide-to-tray, Tauri commands. NSIS / AppImage / deb / dmg bundles.
- `src/` + `index.html` — React + TypeScript + Vite + Tailwind UI for both
  shells; `src/api.ts` picks Tauri `invoke` or HTTP.

## Conventions worth knowing

- **Anthropic error shape.** Everything under `/v1/*` (including auth
  rejections) returns `{"type":"error","error":{type,message}}` via
  `http::anthropic_error`. Admin endpoints use `http::json_error`.
- **Single-shown secrets.** Invite tokens, new API keys and generated admin
  tokens are returned exactly once. Never log them or upstream tokens.
  Admin responses never include token material (account list is metadata).
- **bcrypt off the async workers.** Key lookup, invite redeem, key issue and
  admin verify run in `spawn_blocking`.
- **New admin actions** need an `/admin/api/*` endpoint, a
  `ccm-server admin …` subcommand, and (once the UI covers it) a Tauri
  command, wrapped once in `src/api.ts`.
- **Upstream wire format** (headers, OAuth) changes go in `upstream.rs` /
  `oauth.rs`, with a fake-server test.
- **Config precedence:** defaults → `config.json` → `CCM_*` env; add new
  options to `config.rs` (`Default` impl + `apply_env` when overridable).

## Working in this repo

- Development branch for the current task: `claude/epic-gauss-en0jx3`.
  `main` is the upstream default.
