# CC Manager

一个桌面程序（Tauri）加一个无界面服务端，让局域网里的多台终端共用你已登录的
**Claude Pro/Max** 账号。CC Manager 加密保存各账号的 OAuth token、自动刷新、在账号之间
调度和故障切换，并用它自己签发的终端 Key 转发 Anthropic Messages API 和 OpenAI
Chat Completions 请求。参考 [lbjlaq/Antigravity-Manager](https://github.com/lbjlaq/Antigravity-Manager)，
只针对 Claude。

English: [README.md](./README.md) · 阶段计划：[docs/ROADMAP.zh.md](./docs/ROADMAP.zh.md)

> **范围说明**：仅用于个人实现与测试，只接入你自己的 Claude OAuth 账号，**不支持**
> Anthropic API Key 账号。Anthropic 消费者条款不允许共享或转售 Pro/Max 订阅。

## 功能

- **代理**：`POST /v1/messages`（Anthropic，SSE 流式透传）和 `POST /v1/chat/completions`
  （OpenAI 兼容，支持流式、工具调用、图片）。设置 `ANTHROPIC_BASE_URL` + `ANTHROPIC_API_KEY`
  后原版 `claude` 直接可用。
- **账号**：OAuth 登录（PKCE）；导入 Claude Code 凭据（文件、批量 JSON、macOS 钥匙串）；
  首次启动自动导入本机登录；token 用 AES-256-GCM 加密；每个账号可单独配 HTTP/SOCKS5 出站代理。
- **调度**：优先级 / 加权轮询 / 最少使用；粘性会话保住 prompt cache；429 / 401 / 403 / 5xx
  自动切换账号并冷却；账号分组；按 5 小时 / 7 天额度用量避开快用完的账号。
- **终端**：邀请码自助注册；`sk-cp-…` Key（bcrypt 存储、只显示一次）；终端策略（模型白名单、
  每分钟请求数、每日 token、账号组）；全局禁用模型；模型路由（别名 / 正则）。
- **管理**：React 网页界面（桌面窗口或浏览器）、JSON 接口、命令行、用量统计与搜索、Prometheus `/metrics`。
- **桌面端**：托盘显示当前账号和额度、快速切换账号、启停代理；账号失效或全部限流时发系统通知。
  安装包：NSIS（Windows）、AppImage / deb、dmg。
- **无界面**：`ccm-server` 单个二进制（内嵌网页界面）、Docker 镜像、安装脚本。

## 快速开始

### 桌面端

从 [Releases](https://github.com/joneswapple/cc-Mannger/releases) 下载安装包（或自行编译，见"开发"）。
首次启动会导入本机的 Claude Code 登录，并在 8787 端口启动代理。在「上游账号 → OAuth 登录」
添加更多账号，在「终端」新建终端，在「接入」复制配置片段。

### 无界面

```sh
# 安装（Linux / macOS；Windows 用 scripts/install.ps1），或 cargo build --release -p ccm-server
curl -fsSL https://raw.githubusercontent.com/joneswapple/cc-Mannger/main/scripts/install.sh | sh

ccm-server serve                     # 首次启动打印管理员令牌
# 浏览器打开 http://<主机>:8787/ ，用管理员令牌登录
```

也可以用 Docker：`docker compose up -d`（见 [docker-compose.yml](./docker-compose.yml)），
管理员令牌在 `docker compose logs` 里。

### 接入终端

```sh
# 用邀请码换 Key（邀请码在界面或 `ccm-server admin invite create` 创建）
curl -X POST http://<主机>:8787/register -H "Content-Type: application/json" \
     -d '{"token":"<邀请码>","label":"alice"}'

export ANTHROPIC_BASE_URL=http://<主机>:8787
export ANTHROPIC_API_KEY=sk-cp-...
claude
```

OpenAI 客户端用 `base_url=http://<主机>:8787/v1`，Key 相同。

## 配置

优先级：代码默认值 → `config.json` → `CCM_*` 环境变量。完整示例见
[`config.example.json`](./config.example.json)。桌面端读取 `<应用配置目录>/config.json`；
`ccm-server` 用 `--config` / `CCM_CONFIG` 指定。

| JSON 字段                            | 环境变量                      | 默认值                                         |
| ------------------------------------ | ----------------------------- | ---------------------------------------------- |
| `listen`                             | `CCM_LISTEN`                  | `0.0.0.0:8787`                                 |
| `data_dir`                           | `CCM_DATA_DIR`                | `<系统数据目录>/cc-manager`                    |
| `log_level`                          | `CCM_LOG_LEVEL` / `RUST_LOG`  | `info`                                         |
| `secret_key`                         | `CCM_SECRET_KEY`              | 空 → 使用 / 生成 `data_dir/secret.key`          |
| `admin.initial_token`                | `CCM_ADMIN_TOKEN`             | 空 → 首次启动随机生成并打印                    |
| `upstream.base_url`                  | `CCM_UPSTREAM_BASE_URL`       | `https://api.anthropic.com`                    |
| `upstream.credentials_path`          | `CCM_CREDENTIALS_PATH`        | 自动探测（见下文）                             |
| `upstream.sync_credentials_file`     | `CCM_SYNC_CREDENTIALS_FILE`   | `true`                                         |
| `upstream.oauth_*` / `profile_url`   | —                             | 与 Claude Code CLI 一致，见英文文档            |
| `upstream.usage_url`                 | —                             | 空（不主动轮询，仍从响应头读取额度）           |
| `upstream.usage_poll_secs`           | —                             | `600`                                          |
| `upstream.inject_claude_code_system` | —                             | `true`                                         |
| `pool.strategy`                      | `CCM_POOL_STRATEGY`           | `priority`（可选 `round_robin`、`least_used`） |
| `pool.sticky`                        | `CCM_POOL_STICKY`             | `true`                                         |
| `pool.max_retries`                   | `CCM_POOL_MAX_RETRIES`        | `2`                                            |
| `pool.cooldown_secs`                 | —                             | `60`                                           |
| `pool.quota_soft_limit`              | —                             | `0.95`                                         |
| `routing.background_model`           | —                             | 空（关闭）                                     |
| `routing.background_max_tokens`      | —                             | `512`                                          |

OAuth、profile、用量这些接口都没有公开文档，全部做成配置项，Anthropic 改了可以跟着调。

**加密密钥**：上游 token 用 `secret_key` 或 `data_dir/secret.key` 加密，请和 `cc-manager.db` 一起备份。

**本机登录**：还没有任何账号时，按 `upstream.credentials_path` → `$CLAUDE_CREDENTIALS_PATH` →
`$CLAUDE_CONFIG_DIR/.credentials.json` → `~/.claude/.credentials.json` 的顺序找到 Claude Code
凭据并导入为账号 #1。开启 `sync_credentials_file`（默认）时以该文件为准，本机的 `claude` 可以同时使用。
macOS 用 `ccm-server admin account import --from-keychain` 从钥匙串导入。

## 账号

- **OAuth 登录**：界面（或 `ccm-server admin account login`）打开 Claude 授权页，同意后页面显示
  `code#state`，粘贴回来即可。如果把 `upstream.oauth_redirect_uri` 设为
  `http://<主机>:8787/oauth/callback`（且 client id 允许），登录会自动完成。
- **导入**：任意常见格式的 `.credentials.json`，或它们组成的 JSON 数组（元素可以是
  `{"label","account_group","credentials"}`）。邮箱相同的账号会更新而不是重复添加，重新登录也靠这个
  清除"需重新登录"状态。
- **单个账号**：标签、优先级、权重、分组、代理、启用 / 禁用、置顶、立即刷新、解除冷却、删除。
  配了代理的账号所有请求都只走代理，不会回落直连。

## 调度与故障切换

每个请求只在以下账号里选：启用、属于该终端的账号组（策略里的 `account_group`，不设则全部）、
状态正常、不在冷却中。其中优先用额度低于 `pool.quota_soft_limit` 的；会话之前绑定的账号仍健康就继续用
（会话取 Claude Code 的 `metadata.user_id`，没有则按终端 Key）；否则按 `pool.strategy` 选。

在给客户端写出任何字节之前，失败会换下一个账号（最多再试 `pool.max_retries` 个）：

| 上游结果                            | 处理                                                          |
| ----------------------------------- | ------------------------------------------------------------- |
| 429                                 | 冷却到 `retry-after` / `anthropic-ratelimit-*-reset`（没有则 `pool.cooldown_secs`） |
| 401                                 | 强制刷新 token（没有 refresh token 则标记需重新登录）          |
| 403                                 | 冷却 30 分钟                                                  |
| 500 / 502 / 503 / 504 / 529、网络错误 | 直接换下一个，不冷却                                         |
| 其他                                | 原样返回                                                      |

账号试完时把最后一次上游响应原样返回；所有可用账号都在冷却时直接返回 429 和 `retry-after`，
不再请求上游。冷却时间存在数据库里，重启不丢。

**额度**：每次上游响应的 `anthropic-ratelimit-unified-5h/7d-*` 响应头都会更新该账号的额度快照，
在界面、托盘和 `/metrics` 里显示。设置 `upstream.usage_url` 可以额外定时主动查询。

## OpenAI 兼容

`POST /v1/chat/completions` 与 Messages 双向转换：system / developer 消息、文本和图片
（data URL 或 http URL）、tools 与 `tool_choice`、assistant 的 `tool_calls` 和 `tool` 结果、`stop`、
`max_completion_tokens`、`reasoning_effort`（映射到 effort 档位）、`stream` 与
`stream_options.include_usage`。响应包含 `tool_calls`、`reasoning_content` 和 usage。
用模型路由把 OpenAI 模型名映射过来，例如 `re:^gpt-4` → `claude-sonnet-4-5`。

## 接口与命令行

完整接口表和命令行用法见 [README.md](./README.md#http-api)。常用命令：

```sh
ccm-server admin account login --label 我的 Max
ccm-server admin account import --file ~/.claude/.credentials.json
ccm-server admin account set 2 --priority 10 --group team --proxy socks5://127.0.0.1:1080
ccm-server admin invite create --label alice --ttl-hours 24
ccm-server admin user policy 3 --rpm 10 --daily-tokens 2000000 --group team
ccm-server admin models routes 're:^gpt-4=claude-sonnet-4-5'
ccm-server admin reset-token                      # 忘了管理员令牌时，直接写本地数据库
ccm-server migrate-from-go --go-data-dir ~/.local/share/cliproxy
```

## 运行说明

- **effort 档位**：请求体顶层的 `"effort"` 会补上 `max_tokens` 和 extended thinking
  （auto 8192 · low 4096 · medium 16384+5000 · high 32768+15000 · xhigh 48000+30000 · max 64000+60000）；
  请求里已有的值优先；未知档位返回 400。
- **system 注入**：OAuth Pro/Max 接口要求第一个 system 块是
  `You are Claude Code, Anthropic's official CLI for Claude.`，否则返回误导性的 `rate_limit_error`。
  缺少时自动补上，可用 `upstream.inject_claude_code_system` 关闭。
- **上游错误**：状态码、响应头和响应体原样转发，错误摘要（含 `request_id`）写日志并记入用量。
- **监控**：`/metrics` 按账号和状态码给出请求数、token 计数、账号可用 / 冷却和额度利用率，
  用管理员令牌作为 Bearer 抓取。
- **备份**：备份 `data_dir`（`cc-manager.db` + `secret.key`）。

## 开发

```sh
pnpm install && pnpm build          # 网页界面（编译 ccm-server 时内嵌）
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo run -p ccm-server -- serve    # 无界面
pnpm tauri dev                      # 桌面端；`pnpm tauri build` 打安装包
```

Linux 编译桌面端需要 `libwebkit2gtk-4.1-dev`、`libayatana-appindicator3-dev`、`librsvg2-dev`。
推送 `v*` 标签会触发发布流水线：三平台安装包、`ccm-server` 二进制、`ghcr.io` 镜像，生成草稿 Release。
