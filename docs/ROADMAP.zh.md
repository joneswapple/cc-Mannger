# cliproxy → Claude 多账号管理器 阶段计划

参考对象：[lbjlaq/Antigravity-Manager](https://github.com/lbjlaq/Antigravity-Manager)
（Tauri + React + Rust/Axum 的桌面端多账号中转站）。本计划把它的核心思路移植到
cliproxy，但**只针对 Claude**，并保持本仓库现有的约束：单个 Go 二进制、无 CGO、
SQLite 内嵌、服务端渲染的 `/admin/` 面板、Anthropic 错误格式、配置优先级
（默认值 → YAML → `CLIPROXY_*` 环境变量）。

**范围说明**：本项目仅用于个人实现与测试，只支持 Claude Pro/Max OAuth 账号，
**不实现 Anthropic API Key 账号类型**。

## 1. 功能对照

| Antigravity-Manager                    | cliproxy 现状                         | 目标（Claude 版）                                                                 |
| -------------------------------------- | ------------------------------------- | --------------------------------------------------------------------------------- |
| 多账号池（列表 / 网格视图）            | 只读一个 `~/.claude/.credentials.json` | `accounts` 表，仅支持 Claude Pro/Max OAuth 账号（不支持 API Key）                 |
| OAuth 授权链接 + 回调 / 手动继续       | 依赖宿主机先 `claude /login`          | 内置 PKCE 登录：生成授权链接 → 回调或手动粘贴 code → 自动入库                    |
| 单条 / JSON 批量导入、V1 迁移          | 无                                    | 导入 `.credentials.json`、JSON 批量、macOS Keychain；旧单账号配置自动迁为账号 #1 |
| 配额监控、最佳账号推荐、一键切换       | 无                                    | 采集 5h / 7d 用量与重置时间，面板展示，按剩余额度排序，支持手动置顶 / 固定       |
| 429/401 自动轮换、加权调度、粘性会话   | 无（单账号）                          | 调度器：优先级 / 轮询 / 最低用量 / 粘性会话；失败自动切换 + 冷却                 |
| 403 检测、问题账号跳过                 | 无                                    | 账号健康状态机：active / cooling / needs_reauth / disabled                        |
| 模型路由（系列映射、正则映射）         | 只有 effort 映射与全局禁用模型        | 别名 / 正则改写；后台任务降级到 Haiku；按账号能力过滤                            |
| OpenAI / Anthropic / Gemini 协议       | 仅 Anthropic `/v1/messages`           | 增加 OpenAI `/v1/chat/completions`（之后可选 Responses API）；不做 Gemini        |
| 代理池、防直连泄漏                     | 无                                    | 每账号可配出站 HTTP/SOCKS5 代理，配置了代理的账号禁止回落直连                    |
| 本地加密存储                           | token 写回明文 credentials 文件       | 账号 token 在 SQLite 中用 AES-GCM 加密（密钥文件 / 环境变量）                     |
| CLI 切换账号 / 客户端一键配置          | `cliproxy admin …`                    | `cliproxy admin account …`；输出 Claude Code / OpenAI SDK 客户端配置片段         |
| Tauri 桌面、托盘、HUD                  | —                                     | **不做**。保持 Web 面板 + 单二进制 + Docker                                      |

## 2. 阶段划分

每个阶段都是可独立合并、可回滚的一个 PR，结束时 `go build`、`go vet`、`go test ./...`
全部通过，且旧的单账号部署方式不需要改配置就能继续工作。

### 阶段 0：账号抽象与存储（地基）

目标：把"一个凭据文件"变成"数据库里的若干账号"，行为与现在完全一致。

- `internal/store/migrations/002_accounts.sql`
  - `accounts(id, label, email, org_uuid, plan, priority,
    weight, enabled, status, status_reason, proxy_url, created_at, updated_at)`
  - `account_secrets(account_id, access_token_enc, refresh_token_enc, expires_at, scopes)`
  - `usage_events` 增加 `account_id` 列（可空，兼容旧数据）
- `internal/secret`：AES-GCM 封装；密钥来源 `CLIPROXY_SECRET_KEY` → `data_dir/secret.key`（首次自动生成，0600）。
- `internal/upstream`
  - 新的 `Account` 类型与 `AccountStore`（读写 + 加密）。
  - `OAuthAccountProvider`：从 `ClaudeCLIProvider` 拆出刷新逻辑，存储改为 DB；刷新加单账号互斥锁。
  - `Client.do` 改为接收"已选中的账号"而不是全局 `Provider`。
- 启动迁移：若 `accounts` 为空且能找到旧的 credentials 文件，则导入为账号 #1，
  并保留"回写 credentials 文件"的兼容开关（宿主 `claude` 仍在用同一份 token 时需要）。
- 测试：加密往返；从旧文件导入；fake OAuth server 下的刷新（扩展 `claudecli_test.go`）。

验收：只有一个账号时，`/v1/messages` 的行为和请求头与改造前逐字节一致（现有 `proxy_test.go` 全绿）。

### 阶段 1：账号池调度与故障切换（核心价值）

- `internal/pool`
  - `Scheduler.Pick(ctx, req) (*Account, release func(result))`
  - 策略：`priority`（默认，按优先级 + 健康度）、`round_robin`、`least_used`（按最近 5h 用量）、
    以及叠加在上面的 `sticky`（按终端的 `sk-cp-…` Key / `metadata.user_id` / 会话哈希绑定账号，
    以保住 prompt cache 命中率；绑定账号不可用时才迁移）。
  - 状态机：`active → cooling(until) → active`；`401/invalid_grant → needs_reauth`；`403 → disabled(reason)`。
  - 冷却时长取自 `retry-after` / `anthropic-ratelimit-*-reset` 响应头，缺省指数退避。
- `proxy.go`：在**尚未向客户端写出任何字节之前**遇到 429 / 401 / 403 / 529 / 网络错误，
  换账号重试（最多 N 次，可配置）；SSE 一旦开始输出就不再重试，原样透传。
- `usage_events.account_id` 落库，统计页增加"按账号"汇总。
- 用户 → 账号组绑定（可选）：`policies` 增加 `account_group`，让不同终端只用指定账号。
- 配置：`pool.strategy`、`pool.max_retries`、`pool.sticky_ttl`、`pool.cooldown_default`（均支持 env 覆盖）。
- 测试：fake upstream 返回 429/401 时的切换；粘性绑定；并发下不重复刷新同一账号。

### 阶段 2：账号管理入口（登录 / 导入 / 面板 / CLI）

按 CLAUDE.md 约定，每个管理动作都要同时有 `/admin/api/*` 和 `cliproxy admin …` 两种入口。

- OAuth PKCE 登录（URL、client_id、scope、redirect 全部走配置，因为没有官方文档）：
  - `POST /admin/api/accounts/oauth/start` → 返回授权链接 + state
  - `GET  /admin/oauth/callback`（本机回调）与 `POST /admin/api/accounts/oauth/complete`（手动粘贴 `code#state`）
  - 换 token 后拉取 profile（邮箱、组织、套餐）写入 `accounts`
- 导入：`POST /admin/api/accounts/import`（单个 credentials JSON、JSON 数组批量）；
  CLI 额外支持 `--from-keychain`（macOS 下调 `security`，不引入 CGO）。
- 账号操作：启用 / 禁用 / 改优先级 / 改标签 / 改代理 / 强制刷新 token / 删除 / 置为首选。
- CLI：`cliproxy admin account {list,add,login,import,enable,disable,pin,refresh,rm}`。
- 面板：新增 `templates/accounts.html`（列表 + 状态徽标 + 操作按钮 + 登录弹窗），沿用无框架的内联 `fetch`。
- 安全：token 永不出现在 API 响应、日志、面板 HTML 中，只显示前缀 / 邮箱。

### 阶段 3：配额与健康监控

- 被动采集：从每次上游响应头解析 `anthropic-ratelimit-unified-*`（5h / 7d 利用率、状态、重置时间），
  写入 `account_quota` 快照表（内存缓存 + 定期落库）。
- 主动探测（可选、默认关闭）：定时调用用量接口或发一个极小请求刷新快照；接口地址走配置。
- 调度器使用快照：利用率超过阈值（如 95%）的账号降权，到重置时间自动恢复。
- 面板：账号卡片显示 5h / 7d 进度条、重置倒计时、最近错误；首页显示全局平均剩余、"推荐账号"。
- 告警：账号进入 `needs_reauth` 或全部账号冷却时，日志 + 可选 webhook。

### 阶段 4：模型路由与协议兼容

- 模型映射：`model_routes`（精确别名 + 正则），如 `gpt-4o → claude-sonnet-*`、`*-latest → 具体版本`；
  面板与 CLI 可编辑，存 `global_settings`。
- 后台任务降级：识别 Claude Code 的标题生成 / 话题判断等小请求（小 `max_tokens`、无 tools），
  可配置改写到 Haiku 以节省 Opus/Sonnet 额度。
- 账号能力过滤：记录各账号可用模型（来自 `/v1/models` 或报错学习），调度时排除不支持该模型的账号。
- OpenAI 兼容：`POST /v1/chat/completions`（流式 / 非流式、tools ↔ tool_use、图像输入、usage 映射），
  错误仍需能被 OpenAI SDK 理解；`/v1/models` 同时支持 OpenAI 形状。
- 之后可选：OpenAI Responses API（`/v1/responses`）。

### 阶段 5：运维与发布

- 出站代理：每账号 `proxy_url`（HTTP / SOCKS5，纯 Go 实现），代理失败时不回落直连。
- Dockerfile + docker-compose（`CLIPROXY_*` 环境变量、数据卷）。
- `/metrics`（Prometheus 文本格式，手写即可，不加依赖）：每账号请求数、错误率、冷却次数。
- 客户端配置生成：面板 / CLI 一键输出 Claude Code `settings.json` 片段、环境变量、OpenAI SDK 示例。
- README / README.zh / CLAUDE.md 同步更新。

## 3. 建议顺序与工作量

| 阶段 | 依赖 | 粗略工作量 | 交付物                              |
| ---- | ---- | ---------- | ----------------------------------- |
| 0    | —    | 中         | 账号表、加密、单账号兼容            |
| 1    | 0    | 中-大      | 调度器 + 自动切换（核心）           |
| 2    | 0    | 中         | OAuth 登录、导入、账号面板 / CLI    |
| 3    | 1    | 小-中      | 配额快照、面板进度条、降权          |
| 4    | 1    | 大         | 模型路由、OpenAI 兼容               |
| 5    | 2    | 小         | 代理、Docker、metrics、文档         |

阶段 1 和阶段 2 在阶段 0 之后可以并行。

## 4. 风险与需要确认的问题

1. **服务条款风险**：Anthropic 消费者条款不允许共享 / 转售 Pro/Max 订阅，多账号轮换以绕过
   单账号限额可能导致封号。本项目定位为个人实现与测试用途，只接入自己的 OAuth 账号，
   不做 API Key 账号。
2. **非官方接口**：OAuth 授权 / token / profile / 用量接口和 `anthropic-ratelimit-unified-*`
   响应头都没有公开文档，全部做成可配置项，并用 fake server 测试，随时可能需要跟进调整。
3. **与宿主 `claude` 共用 token**：同一个 refresh token 被两边轮换会互相踢掉。阶段 0 保留
   "回写 credentials 文件"兼容模式；新登录的账号建议只给 cliproxy 用。
4. **待你确认**：
   - 是否需要 OpenAI 兼容（阶段 4 的大头），还是只服务 Claude Code 客户端？
   - 默认调度策略选"粘性优先"（保 cache）还是"用量均衡"？
   - 是否需要 Bedrock / Vertex 账号（`CredentialProvider` 接口已预留）？
