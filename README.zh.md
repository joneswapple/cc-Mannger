# CC Manager

一个桌面程序（Tauri）加一个无界面服务端，让局域网里的多台终端共用你已登录的
**Claude Pro/Max** 账号。你只需在主机上登录一次；CC Manager 会加密保存 OAuth
token、按需刷新，并用它自己签发的终端 Key 转发 Anthropic Messages API 请求。
参考 [lbjlaq/Antigravity-Manager](https://github.com/lbjlaq/Antigravity-Manager)，
只针对 Claude。

English: [README.md](./README.md) · 阶段计划：[docs/ROADMAP.zh.md](./docs/ROADMAP.zh.md)

> **范围说明**：仅用于个人实现与测试，只接入你自己的 Claude OAuth 账号，
> **不支持** Anthropic API Key 账号。Anthropic 消费者条款不允许共享或转售
> Pro/Max 订阅。

## 功能

- `POST /v1/messages`：兼容 Anthropic 的接口，SSE 流式透传。设置
  `ANTHROPIC_BASE_URL` + `ANTHROPIC_API_KEY` 后，原版 `claude` 直接可用。
- 邀请码自助注册：每台终端用邀请码换一个自己的 `sk-cp-…` Key（bcrypt 哈希存储，只显示一次）。
- 用户级策略：启用 / 禁用、模型白名单、每分钟请求数、每日 token 额度；另有全局禁用模型列表。
- 每次请求记录用量（token、耗时、状态码、上游账号），可按天 / 用户 / 模型 / 账号汇总。
- 上游账号存在 SQLite 中，token 用 AES-256-GCM 加密；首次启动自动导入主机上 Claude Code 的登录。
- 多账号调度（优先级 / 加权轮询 / 最少使用）、保住 prompt cache 的粘性会话、
  429 / 401 / 403 / 5xx 自动切换账号并冷却、账号分组。
- 一套 Rust 核心，两种外壳：**桌面程序**（窗口 + 托盘，代理在程序内运行）和
  **`ccm-server`**（无界面，适合服务器 / Docker）。

## 快速开始（无界面）

```sh
# 1. 编译
cargo build --release -p ccm-server

# 2. 确认主机上的 Claude Code 已登录（`claude /login`），
#    凭据默认在 ~/.claude/.credentials.json

# 3. 启动。首次启动会打印管理员令牌，请立即保存
./target/release/ccm-server serve        # 或：--config ./config.json

# 4. 创建邀请码（admin 子命令调用正在运行的服务）
export CCM_ADMIN_TOKEN=<管理员令牌>
./target/release/ccm-server admin invite create --label alice-laptop

# 5. 在局域网终端上用邀请码换 Key
curl -X POST http://<主机>:8787/register \
     -H "Content-Type: application/json" \
     -d '{"token":"<邀请码>","label":"alice"}'

# 6. 让 Claude Code 走代理
export ANTHROPIC_BASE_URL=http://<主机>:8787
export ANTHROPIC_API_KEY=sk-cp-...
claude
```

## 桌面程序

```sh
pnpm install
pnpm tauri dev      # 开发
pnpm tauri build    # 打包：Windows NSIS、Linux AppImage/deb、macOS dmg
```

Linux 下编译需要 `libwebkit2gtk-4.1-dev`、`libayatana-appindicator3-dev`、
`librsvg2-dev`。关闭窗口后代理继续在托盘里运行。桌面程序读取
`<应用配置目录>/config.json`，与 `ccm-server` 共用同一个数据目录。

## 配置

优先级：代码默认值 → `config.json` → `CCM_*` 环境变量。完整示例见
[`config.example.json`](./config.example.json)。

| JSON 字段                         | 环境变量                         | 默认值                                         |
| --------------------------------- | -------------------------------- | ---------------------------------------------- |
| `listen`                          | `CCM_LISTEN`                     | `0.0.0.0:8787`                                 |
| `data_dir`                        | `CCM_DATA_DIR`                   | `<系统数据目录>/cc-manager`                    |
| `log_level`                       | `CCM_LOG_LEVEL`（或 `RUST_LOG`） | `info`                                         |
| `secret_key`                      | `CCM_SECRET_KEY`                 | 空 → 使用 / 生成 `data_dir/secret.key`          |
| `upstream.base_url`               | `CCM_UPSTREAM_BASE_URL`          | `https://api.anthropic.com`                    |
| `upstream.credentials_path`       | `CCM_CREDENTIALS_PATH`           | 自动探测（见下文）                             |
| `upstream.sync_credentials_file`  | `CCM_SYNC_CREDENTIALS_FILE`      | `true`                                         |
| `upstream.oauth_token_url`        | —                                | `https://console.anthropic.com/v1/oauth/token` |
| `upstream.oauth_client_id`        | —                                | `9d1c250a-e61b-44d9-88ed-5944d1962f5e`         |
| `upstream.anthropic_beta`         | —                                | `claude-code-20250219,oauth-2025-04-20`        |
| `upstream.user_agent`             | —                                | `claude-cli/1.0 (cc-manager)`                  |
| `upstream.inject_claude_code_system` | —                             | `true`                                         |
| `upstream.effort_levels`          | —                                | 见"effort 档位"                                |
| `pool.strategy`                   | `CCM_POOL_STRATEGY`              | `priority`（可选 `round_robin`、`least_used`） |
| `pool.sticky`                     | `CCM_POOL_STICKY`                | `true`                                         |
| `pool.sticky_ttl_secs`            | —                                | `3600`                                         |
| `pool.max_retries`                | `CCM_POOL_MAX_RETRIES`           | `2`                                            |
| `pool.cooldown_secs`              | —                                | `60`                                           |
| `admin.initial_token`             | `CCM_ADMIN_TOKEN`                | 空 → 首次启动随机生成并打印                    |

`ccm-server` 也接受 `--config` / `CCM_CONFIG` 指定配置文件路径。

**账号与加密密钥**：上游 token 用 `secret_key` 或 `data_dir/secret.key` 加密。
请把这个密钥和 `cc-manager.db` 一起备份，丢了密钥就无法解密已保存的 token。
还没有任何账号时，会把 Claude Code 凭据文件导入为账号 #1，查找顺序：
`upstream.credentials_path` → `$CLAUDE_CREDENTIALS_PATH` →
`$CLAUDE_CONFIG_DIR/.credentials.json` → `~/.claude/.credentials.json`。
开启 `sync_credentials_file`（默认）时，这个文件始终以它为准（刷新前重读、刷新后回写），
主机上的 `claude` 可以和 CC Manager 同时使用。macOS 需要先从钥匙串导出：
`security find-generic-password -s "Claude Code-credentials" -w > ~/.claude/.credentials.json`。

## HTTP 接口

管理接口里的时间都是 unix 秒。

| 方法 | 路径                                      | 认证          | 说明                                                  |
| ---- | ----------------------------------------- | ------------- | ----------------------------------------------------- |
| `POST` | `/v1/messages`                          | `x-api-key`   | Anthropic Messages 接口（SSE + JSON）                 |
| `POST` | `/v1/messages/count_tokens`             | `x-api-key`   | 计算 token，透传；不受策略限制、不计统计              |
| `GET`  | `/v1/models`                            | `x-api-key`   | 从缓存的上游模型目录返回                              |
| `POST` | `/register`                             | 邀请码（body）| `{token, label}` → `{api_key, user_id}`               |
| `GET`  | `/healthz`                              | —             | 存活检查：`{status, version}`                         |
| `GET`  | `/admin/api/status`                     | 管理员        | 版本、账号数、上游来源、模型目录状态                  |
| `GET` `POST` | `/admin/api/users`                | 管理员        | 用户列表（含 Key 与策略，不含哈希）/ 创建用户         |
| `POST` | `/admin/api/users/{id}/{enable,disable}` | 管理员       | 启用 / 禁用用户                                       |
| `GET` `POST` | `/admin/api/users/{id}/policy`    | 管理员        | 读取 / 替换策略                                       |
| `POST` | `/admin/api/keys/issue`                 | 管理员        | `{user_id}` → `{api_key, key_id}`                     |
| `POST` | `/admin/api/keys/{id}/revoke`           | 管理员        | 吊销 Key                                              |
| `GET` `POST` | `/admin/api/invites`              | 管理员        | 邀请码列表 / 创建                                     |
| `POST` | `/admin/api/invites/{hash}/delete`      | 管理员        | 删除未使用的邀请码                                    |
| `GET`  | `/admin/api/accounts`                   | 管理员        | 上游账号（只有元数据，绝不返回 token）、调度配置、`now` |
| `POST` | `/admin/api/accounts/{id}`              | 管理员        | 修改 `{label, priority, weight, enabled, account_group, clear_cooldown}` |
| `GET`  | `/admin/api/settings`                   | 管理员        | 禁用模型、模型目录、上游请求参数                      |
| `POST` | `/admin/api/settings/disabled-models`   | 管理员        | 替换全局禁用模型列表                                  |
| `POST` | `/admin/api/settings/refresh-models`    | 管理员        | 立即刷新模型目录                                      |
| `GET`  | `/admin/api/stats?days=N`               | 管理员        | 按天 / 用户 / 模型 / 账号汇总                         |
| `GET`  | `/admin/api/usage`                      | 管理员        | 用量搜索，参数同英文文档                              |

管理员认证：`Authorization: Bearer <令牌>`、`x-admin-token` 或 `ccm_admin` cookie。
`/v1/*` 的错误统一是 Anthropic 格式 `{"type":"error","error":{"type","message"}}`；
管理接口的错误是 `{"error": "..."}`。

## 命令行

```sh
ccm-server serve                               # 默认命令
ccm-server config                              # 打印生效配置（隐去密钥）

# 管理（需要 CCM_ADMIN_TOKEN；CCM_ADMIN_URL 默认 http://127.0.0.1:8787）
ccm-server admin status
ccm-server admin invite create --label alice --ttl-hours 24
ccm-server admin user list | create --label x | enable 3 | disable 3
ccm-server admin user policy 3 --allow claude-sonnet-4-5 --rpm 10 --daily-tokens 2000000
ccm-server admin key issue 3 | revoke 7
ccm-server admin account list
ccm-server admin account set 2 --priority 10 --weight 3 --group team --clear-cooldown
ccm-server admin user policy 3 --group team         # 把终端绑定到账号组
ccm-server admin models list | disable claude-opus-4-1 | refresh
ccm-server admin stats --days 7
ccm-server admin usage --status error
ccm-server admin reset-token [--token 新令牌]   # 直接写本地数据库

# effort 档位
ccm-server effort list
ccm-server effort expand --tier high
```

**忘了管理员令牌**：在主机上执行 `ccm-server admin reset-token`（正在运行的服务一分钟内生效），
或者用 `admin.initial_token` / `CCM_ADMIN_TOKEN` 固定一个，每次启动都会写入。

## 从 Go 版 cliproxy 迁移

```sh
ccm-server migrate-from-go --go-data-dir ~/.local/share/cliproxy
```

把用户、Key、邀请码、策略、管理员令牌、禁用模型和用量历史复制到一个**全新的**
CC Manager 数据目录，ID 保持不变，所以原来的 `sk-cp-…` Key 和管理员令牌可以继续用。
上游账号用 Go 版的 `secret.key`（或 `--go-secret-key`）解密后重新加密。

## 多账号

所有启用的账号都是候选。每个请求：

1. 先筛选：属于该用户的账号组（用户策略里的 `account_group`，不设则所有账号都可用）、
   状态为 `active`、不在冷却中。
2. 会话之前绑定的账号仍然健康就继续用它（会话取 Claude Code 的 `metadata.user_id`，
   没有则按终端 Key），这样 Anthropic 的 prompt cache 能持续命中。
3. 否则按 `pool.strategy` 选：`priority`（优先级最高，其次最早添加）、
   `round_robin`（按 `weight` 平滑加权轮询）、`least_used`（近 5 小时 token 用量最少）。

在向客户端写出任何字节之前，失败的尝试会换下一个账号重试，最多再试 `pool.max_retries` 次：

| 上游结果                     | 处理                                                          |
| ---------------------------- | ------------------------------------------------------------- |
| 429                          | 冷却到 `retry-after` / `anthropic-ratelimit-*-reset`（没有则 `pool.cooldown_secs`），换下一个 |
| 401                          | 强制刷新 token（没有 refresh token 则标记 `needs_reauth`），换下一个 |
| 403                          | 冷却 30 分钟，换下一个                                        |
| 500 / 502 / 503 / 504 / 529、网络错误 | 直接换下一个，不冷却                                  |
| 其他                         | 原样返回                                                      |

账号用完时，把最后一次上游响应原样返回。如果所有可用账号都已在冷却，直接返回 429
`rate_limit_error` 并带上最早恢复时间的 `retry-after`，不再请求上游。冷却时间存在数据库里，
重启后依然有效。每次失败的尝试都会记入用量，错误以 `failover: …` 开头。

## 运行说明

- **effort 档位**：请求体顶层可以带 `"effort"`，自动补上 `max_tokens` 和 extended thinking；
  转发前会删掉该字段；请求里已有的 `max_tokens` / `thinking` 优先；未知档位返回 400。
  默认档位：auto 8192 / low 4096 / medium 16384+5000 / high 32768+15000 /
  xhigh 48000+30000 / max 64000+60000。
- **system 注入**：OAuth Pro/Max 接口要求第一个 system 块是
  `You are Claude Code, Anthropic's official CLI for Claude.`，否则会返回误导性的
  `rate_limit_error: "Error"`。缺少时自动补上；已有的请求原样透传。
  可用 `upstream.inject_claude_code_system: false` 关闭。
- **上游错误**：状态码、响应头（`retry-after`、`anthropic-ratelimit-*`）和响应体原样转发；
  错误类型、消息和 `request_id` 会写日志并存入 `usage_events.error`。
- **OAuth 刷新**：过期前 60 秒内自动刷新；refresh token 被拒时账号标记为 `needs_reauth`。
- **备份**：备份 `data_dir`（`cc-manager.db` + `secret.key`）。

## 开发

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
pnpm install && pnpm build
```
