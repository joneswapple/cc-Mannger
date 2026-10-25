# CC Manager（Claude 多账号管理器）阶段计划

参考对象：[lbjlaq/Antigravity-Manager](https://github.com/lbjlaq/Antigravity-Manager)。
本项目沿用它的技术栈与形态，**只针对 Claude**，用来取代仓库里现有的 Go 版 cliproxy：

| 层           | 技术                                                                 |
| ------------ | -------------------------------------------------------------------- |
| 桌面外壳     | Tauri v2（Rust）：窗口、系统托盘、开机自启、单实例                  |
| 代理 / 核心  | Rust：Axum + Tokio + reqwest（rustls）+ rusqlite（内置 SQLite）     |
| 前端         | React + TypeScript + Vite + Tailwind CSS（HTML 入口）               |
| 无界面模式   | 同一核心库编出的 `ccm-server` 二进制，浏览器访问管理面板，适合 Docker |
| 打包         | Tauri bundler：Windows NSIS 安装包（自定义 NSIS 钩子）、AppImage / deb、dmg |
| 脚本         | Shell / PowerShell 安装脚本、Docker、GitHub Actions 发布流水线       |

**范围说明**：仅用于个人实现与测试，只支持 Claude Pro/Max OAuth 账号，
**不实现 Anthropic API Key 账号类型**。

## 1. 目录结构

```
Cargo.toml                 # Rust workspace
crates/ccm-core/           # 核心库：存储、加密、账号、OAuth、代理、调度（无 GUI 依赖，可单测）
crates/ccm-server/         # 无界面服务端二进制（Docker / 浏览器模式）
src-tauri/                 # Tauri 桌面应用，依赖 ccm-core；NSIS 钩子在 src-tauri/windows/
src/                       # React + TS 前端（桌面和无界面模式共用）
index.html, package.json   # Vite 入口
scripts/                   # install.sh / install.ps1 等
```

**进度**：R0、R1、R2 已完成，Go 版 cliproxy 已删除（旧数据用 `ccm-server migrate-from-go` 迁移）。

## 2. 功能对照

| Antigravity-Manager                    | Claude 版目标                                                                     |
| -------------------------------------- | --------------------------------------------------------------------------------- |
| 多账号池（列表 / 网格视图）            | `accounts` 表，仅 Claude Pro/Max OAuth 账号；React 列表 / 卡片视图                |
| OAuth 授权链接 + 回调 / 手动继续       | 内置 PKCE 登录：生成授权链接 → 本地回调或手动粘贴 code → 自动入库                |
| 单条 / JSON 批量导入、旧版迁移         | 导入 `.credentials.json`、JSON 批量、macOS Keychain、从 Go 版 cliproxy 数据库迁移 |
| 配额监控、最佳账号推荐、一键切换       | 采集 5h / 7d 用量与重置时间；面板与托盘显示；按剩余额度排序；手动置顶            |
| 429/401 自动轮换、加权调度、粘性会话   | 调度器：优先级 / 轮询 / 最低用量 / 粘性会话；失败自动切换 + 冷却                 |
| 403 检测、问题账号跳过                 | 账号状态机：active / cooling / needs_reauth / disabled                            |
| 模型路由（系列映射、正则映射）         | 别名 / 正则改写；后台小请求降级到 Haiku；按账号能力过滤                          |
| OpenAI / Anthropic / Gemini 协议       | Anthropic `/v1/messages` + OpenAI `/v1/chat/completions`；不做 Gemini             |
| 代理池、防直连泄漏                     | 每账号出站 HTTP/SOCKS5 代理，代理失败不回落直连                                  |
| 本地加密存储                           | token 用 AES-256-GCM 加密存 SQLite，密钥文件或环境变量                            |
| API Key 与管理密码分离                 | 终端用 `sk-cp-…` Key（邀请码自助注册）；管理面板用独立管理员令牌                  |
| 托盘、菜单栏额度                       | Tauri 托盘：当前账号、额度、切换账号、启停代理                                   |
| Docker / 无界面 Web 模式               | `ccm-server` + Dockerfile + docker-compose                                        |
| NSIS / AppImage / dmg / 安装脚本       | Tauri bundler + 自定义 NSIS 钩子 + install.sh / install.ps1                       |

## 3. 阶段划分

每个阶段一个可独立合并的提交组；结束时 `cargo fmt --check`、`cargo clippy -D warnings`、
`cargo test --workspace`、`pnpm build`、`pnpm typecheck` 全部通过。

### R0：脚手架与数据层（已完成）

- Cargo workspace；`ccm-core` 实现：
  - 配置：默认值 → `config.json`（应用数据目录）→ `CCM_*` 环境变量。
  - 存储：rusqlite（bundled），内嵌 SQL 迁移 + `schema_migrations` 记录。
  - 加密：AES-256-GCM，按账号与字段绑定 AAD；密钥来自 `CCM_SECRET_KEY` 或数据目录下自动生成的 `secret.key`（0600）。
  - 账号：增删改查、加密读写 token、状态字段。
  - OAuth：刷新 token、`invalid_grant` 时重读重试并标记 `needs_reauth`；与 Claude Code `.credentials.json` 双向同步；首次启动自动导入为账号 #1。
- `ccm-server`：读取配置、打开数据库、导入账号，提供 `/healthz`。
- `src-tauri`：最小桌面外壳（窗口 + 托盘占位），内嵌启动核心服务。
- 前端：Vite + React + TS + Tailwind 骨架，调用 `/healthz` 显示服务状态。
- 测试：加密往返、迁移幂等、导入、fake OAuth server 下的刷新 / 同步 / needs_reauth。

### R1：代理功能与 Go 版对齐，然后删除 Go 代码（已完成）

- Axum：`POST /v1/messages`（SSE 逐行透传 + 从 `message_start` / `message_delta` 抓 usage）、
  `/v1/messages/count_tokens`、`/v1/models`（缓存目录）。
- 请求头注入（Bearer、`anthropic-version`、`anthropic-beta` 合并、User-Agent 透传规则）、
  Claude Code system 块注入、effort 档位映射。
- 终端 Key（`sk-cp-…`，bcrypt + 8 位前缀索引 + 60s 缓存）、邀请码、管理员令牌。
- 策略：模型白名单、全局禁用、RPM、每日 token 额度；统计异步落库与汇总查询。
- 管理 JSON API `/admin/api/*`；错误统一为 Anthropic 格式。
- 从 Go 版数据库迁移（用户、Key、策略、统计、账号）。
- 把 Go 版的测试用例逐条移植；全部通过后删除 `cmd/`、`internal/`、`go.mod`，更新 README / CLAUDE.md。

### R2：账号池调度与故障切换（已完成）

- 调度策略：`priority` / `round_robin` / `least_used`，叠加 `sticky`（按终端 Key / `metadata.user_id` 绑定账号，保住 prompt cache）。
- 在尚未向客户端写出字节前，遇到 429 / 401 / 403 / 529 / 网络错误就换账号重试；冷却时长取自 `retry-after` / 限额重置头。
- 统计增加"按账号"维度；终端可绑定账号组。
- 实现说明：403 没有直接禁用账号，而是冷却 30 分钟（避免一次误判就要手动恢复）；
  所有可用账号都在冷却时本地直接返回 429 + `retry-after`；冷却存在 `accounts.cooldown_until`，重启不丢。

### R3：账号管理界面（登录 / 导入 / 切换）

- OAuth PKCE 登录：授权链接、本地回调端口、手动粘贴 `code#state`；拉取邮箱 / 组织 / 套餐。
- 导入：credentials JSON、批量 JSON、macOS Keychain。
- React 页面：账号列表 / 卡片、启用禁用、优先级、标签、代理、强制刷新、删除、置顶；
  用户 / 邀请码 / Key / 策略 / 统计页（替代 Go 版的服务端渲染面板）。
- 桌面端通过 Tauri command 调用，无界面模式走 `/admin/api/*`，前端共用一套 API 封装。
- CLI：`ccm-server account …` / `admin …` 子命令（clap）。

### R4：额度监控与托盘

- 被动采集上游响应头里的 5h / 7d 利用率与重置时间，存快照；可选定时主动探测。
- 调度器按利用率降权，重置后恢复。
- 面板：进度条、重置倒计时、推荐账号；托盘：当前账号 + 额度、快速切换、启停代理。
- 账号失效或全部冷却时发系统通知。

### R5：模型路由与 OpenAI 兼容

- 别名 / 正则模型映射；后台小请求降级到 Haiku；按账号可用模型过滤。
- OpenAI `/v1/chat/completions`（流式 / 非流式、tools、图片输入、usage 映射）。

### R6：打包与发布

- Tauri bundler：Windows NSIS（中英文安装界面、`installerHooks` 处理旧版残留和防火墙规则）、
  Linux AppImage / deb、macOS dmg。
- `scripts/install.sh`、`scripts/install.ps1`；Dockerfile + docker-compose（`ccm-server`）。
- GitHub Actions：三平台构建矩阵、自动发布 Release。
- 每账号出站代理（HTTP / SOCKS5）、`/metrics`、客户端配置一键生成（Claude Code `settings.json`、环境变量）。

## 4. 风险与说明

1. **服务条款**：Anthropic 消费者条款不允许共享 / 转售 Pro/Max 订阅，多账号轮换可能导致封号。
   本项目定位为个人实现与测试，只接入自己的 OAuth 账号。
2. **非官方接口**：OAuth 授权 / token / profile 接口和限额响应头没有公开文档，全部做成配置项，
   用 fake server 测试。
3. **与宿主 `claude` 共用 token**：同一个 refresh token 被两边轮换会互相踢掉，所以保留
   credentials 文件同步模式；新登录的账号建议只给本程序用。
4. **构建环境**：Tauri 在 Linux 上需要 WebKitGTK 等系统库；云端开发环境只保证 `ccm-core` /
   `ccm-server` / 前端能编译测试，桌面包由 CI 三平台构建。
