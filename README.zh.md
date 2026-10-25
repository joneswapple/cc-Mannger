# cliproxy（中文文档）

cliproxy 是一个用 Go 编写的小型代理服务，让局域网内多台终端共享同一台主机上
**已经登录** 的 Claude Code CLI 会话。运维人员只需在一台主机上执行
`claude /login`，cliproxy 会读取该主机的 OAuth 凭据并按需自动刷新，再用它代
理来自其它终端、各自独立 API Key 的 Anthropic Messages 请求。

英文版：[README.md](./README.md)

## 功能

- `POST /v1/messages` —— 与 Anthropic 兼容的 Messages 接口，完整透传 SSE 流。
  客户端只需设置 `ANTHROPIC_BASE_URL` 与 `ANTHROPIC_API_KEY`，未经修改的
  `claude` CLI 就能直接走代理。
- 邀请码自助注册：每台终端凭一次性邀请码换取自己的 `sk-cp-…` 密钥（数据库中
  仅保存 bcrypt 哈希，明文仅展示一次）。
- 用户级策略：启用/停用、允许模型白名单、每分钟请求数（RPM）、每日 token 配
  额；另叠加一份全局禁用模型黑名单。
- 用量统计：每次请求记录 token 数、延迟、状态码，并按日 / 用户 / 模型聚合。
- 内置 HTML 控制台（路由 `/admin/`），可管理邀请码、用户、密钥、策略，并查
  看用量。
- 单二进制，无 CGO 依赖，SQLite 存储与前端资源全部嵌入。

## 快速开始

```sh
# 1. 编译
go build -o cliproxy ./cmd/cliproxy

# 2. 在主机上确保已登录 Claude Code：
#    claude /login   （或运行 claude 走 OAuth 浏览器流程）
#    凭据默认保存在 ~/.claude/.credentials.json

# 3. 复制示例配置并按需修改
cp config.example.yaml config.yaml

# 4. 启动代理。首次运行会向 stdout 打印一次性管理员 token，请立即保存。
./cliproxy serve --config ./config.yaml

# 5. 用 token 登录 http://<host>:8787/admin/ ，或调用管理 API 建邀请码：
curl -X POST http://127.0.0.1:8787/admin/api/invites \
     -H "Authorization: Bearer <admin-token>" \
     -H "Content-Type: application/json" \
     -d '{"label":"alice-laptop","ttl_hours":24}'

# 6. 在终端用户机上拿邀请码兑换 API Key：
curl -X POST http://<host>:8787/register \
     -H "Content-Type: application/json" \
     -d '{"token":"<invite-token>","label":"alice"}'

# 7. 终端用户把 Claude Code 指向代理即可：
export ANTHROPIC_BASE_URL=http://<host>:8787
export ANTHROPIC_API_KEY=sk-cp-...
claude
```

## 配置文件

仓库根目录提供了带注释的示例 [`config.example.yaml`](./config.example.yaml)。
把它复制为 `config.yaml`（或任意 `--config` 指定的路径）后编辑即可。优先级：
代码默认值 → YAML 文件 → `CLIPROXY_*` 环境变量。

| YAML 字段                       | 环境变量                       | 默认值                                                    |
| ------------------------------ | ------------------------------ | -------------------------------------------------------- |
| `listen`                       | `CLIPROXY_LISTEN`              | `0.0.0.0:8787`                                           |
| `data_dir`                     | `CLIPROXY_DATA_DIR`            | `~/.local/share/cliproxy`                                |
| `log_level`                    | `CLIPROXY_LOG_LEVEL`           | `info`                                                   |
| `upstream.base_url`            | `CLIPROXY_UPSTREAM_BASE_URL`   | `https://api.anthropic.com`                              |
| `upstream.credentials_path`    | `CLIPROXY_CREDENTIALS_PATH`    | 自动探测（见下文）                                        |
| `upstream.oauth_token_url`     | —                              | `https://console.anthropic.com/v1/oauth/token`           |
| `upstream.oauth_client_id`     | —                              | `9d1c250a-e61b-44d9-88ed-5944d1962f5e`                   |
| `upstream.anthropic_beta`      | —                              | `oauth-2025-04-20`                                       |
| `upstream.user_agent`          | —                              | `claude-cli/1.0 (cliproxy)`                              |
| `upstream.sync_credentials_file` | `CLIPROXY_SYNC_CREDENTIALS_FILE` | `true`                                               |
| `secret_key`                   | `CLIPROXY_SECRET_KEY`          | 空（使用 / 生成 `data_dir/secret.key`）                   |
| `admin.initial_token`          | `CLIPROXY_ADMIN_TOKEN`         | 空（首启动时随机生成）                                    |

凭据文件查找顺序：

1. 配置文件中的 `upstream.credentials_path` / 环境变量 `CLIPROXY_CREDENTIALS_PATH`
2. 环境变量 `CLAUDE_CREDENTIALS_PATH`
3. `$CLAUDE_CONFIG_DIR/.credentials.json`
4. `$HOME/.claude/.credentials.json`

macOS 上 Claude Code 把 OAuth 凭据存在 Keychain 里。在 cliproxy 直接支持
Keychain 之前，先把它导出成文件再用 `upstream.credentials_path` 指过去：

```sh
security find-generic-password -s "Claude Code-credentials" -w \
  > ~/.claude/.credentials.json
```

## HTTP 接口

| 方法    | 路径                                          | 鉴权                | 说明                                                  |
| ------- | --------------------------------------------- | ------------------- | ----------------------------------------------------- |
| `POST`  | `/v1/messages`                                | `x-api-key`         | Anthropic 兼容 Messages 接口（SSE 与 JSON）。          |
| `POST`  | `/v1/messages/count_tokens`                   | `x-api-key`         | Token 计数转发；同样的鉴权与系统块注入，不计入用量统计。 |
| `GET`   | `/v1/models`                                  | `x-api-key`         | 从缓存的上游 catalog 返回模型清单。                    |
| `POST`  | `/register`                                   | 邀请码（请求体）    | 用一次性邀请码换取 `sk-cp-…` API Key。                 |
| `GET`   | `/healthz`                                    | —                   | 健康检查。                                            |
| `GET`   | `/admin/login`                                | —                   | 登录页。                                              |
| `POST`  | `/admin/login`                                | 表单（`token`）     | 写入 `cliproxy_admin` Cookie。                         |
| `POST`  | `/admin/logout`                               | Cookie              | 退出登录。                                            |
| `GET`   | `/admin/users` `/invites` `/stats` `/settings` | Cookie 或 Bearer   | 控制台页面。                                          |
| `POST`  | `/admin/api/invites`                          | Cookie 或 Bearer    | 创建邀请码 `{label, ttl_hours}`。                      |
| `POST`  | `/admin/api/invites/{hash}/delete`            | Cookie 或 Bearer    | 删除未兑换的邀请码。                                  |
| `POST`  | `/admin/api/users/{id}/{enable,disable}`      | Cookie 或 Bearer    | 启用/停用用户。                                       |
| `POST`  | `/admin/api/users/{id}/policy`                | Cookie 或 Bearer    | 设置用户策略。                                        |
| `POST`  | `/admin/api/keys/issue`                       | Cookie 或 Bearer    | 为已存在的用户签发新 Key。                            |
| `POST`  | `/admin/api/keys/{id}/revoke`                 | Cookie 或 Bearer    | 吊销 Key。                                            |
| `POST`  | `/admin/api/settings/disabled-models`         | Cookie 或 Bearer    | 设置全局禁用模型列表（JSON `{"models":[...]}`）。     |
| `POST`  | `/admin/api/settings/refresh-models`          | Cookie 或 Bearer    | 立即重新拉取上游模型清单。                            |
| `GET`   | `/admin/api/stats?days=N`                     | Cookie 或 Bearer    | 最近 `N` 天的用量聚合。                               |
| `GET`   | `/admin/api/usage`                            | Cookie 或 Bearer    | 可搜索的用量事件。Query：`q`（模糊匹配 user/model/error）、`user_id`、`model`、`status`（`ok`/`error`/`4xx`/`5xx`/字面码）、`since`/`until`（`YYYY-MM-DDTHH:MM`）、`min_tokens`、`only_error`、`sort`（`time_desc`/`time_asc`/`tokens_desc`/`tokens_asc`/`latency_desc`/`latency_asc`）、`page`、`page_size`。 |

`/v1/messages` 的所有错误都按 Anthropic 标准结构返回：

```json
{ "type": "error", "error": { "type": "...", "message": "..." } }
```

未经修改的 `claude` 客户端因此能正常解析。

## Effort dry-run CLI

`cliproxy effort` 可以本地预览 effort 分级展开，不需要起服务。它直接调用代
理运行时同一份 `applyEffort` 逻辑去吃当前配置里的 `effort_levels`。

```sh
# 看当前配置定义了哪些档
cliproxy effort list

# 预览单个档位展开成的字段
cliproxy effort expand --tier high

# 预览整个 body 改写后的样子
cliproxy effort expand --body '{"model":"claude-opus-4-7","effort":"max","messages":[]}'

# 也支持从 stdin 接收 body
echo '{"model":"x","effort":"medium"}' | cliproxy effort expand
```

## 命令行管理

大部分管理子命令是对管理 API 的薄封装，需要先设置 `CLIPROXY_ADMIN_TOKEN`
（如果代理不在 `127.0.0.1:8787`，再设置 `CLIPROXY_ADMIN_URL`）：

```sh
cliproxy admin invite create --label alice --ttl-hours 24
cliproxy admin user disable 3
cliproxy admin user enable 3
cliproxy admin key revoke 7
```

唯一例外是 **`cliproxy admin reset-token`**：它直接写本地 SQLite 数据库，不
经过 HTTP，所以 **必须在服务停掉之后** 再执行。

## 找回管理员 Token

cliproxy 首次启动时会把 admin token 输出到 stdout，**仅此一次**。如果丢失了，
所有 `/admin/*` 路由都会跳到 `/admin/login?next=…`，无法登录。两种恢复方式：

**方式 1：用 CLI 重置**

```sh
# 先停掉服务
./cliproxy --config ./config.yaml admin reset-token
# 也可以指定具体值
./cliproxy --config ./config.yaml admin reset-token --token my-new-secret
# 重新启动
./cliproxy --config ./config.yaml serve
```

**方式 2：在配置文件中固定**

把 `admin.initial_token` 写进 `config.yaml`（或设置 `CLIPROXY_ADMIN_TOKEN`
环境变量）。每次启动都会按这个值安装 token，只要保住配置文件就不会被锁在
门外：

```yaml
admin:
  initial_token: "your-long-random-string"
```

## 运维提示

- **把 cliproxy 当 `claude` 的后端使用**：未经修改的 Claude Code CLI 可以
  直接指过来（`ANTHROPIC_BASE_URL=http://proxy.lan:8787`），关键点：
  - `GET /v1/models` 由 cliproxy 自己回（取自定时刷新的 catalog 缓存），
    `claude` 启动探测模型时无需再绕一次 `api.anthropic.com`。
  - 入站 `anthropic-beta` 与配置默认值**合并**去重后再发上游，所以
    `claude` 自带的 `interleaved-thinking-…`、
    `fine-grained-tool-streaming-…` 等 feature flag 会和必需的
    `claude-code-…,oauth-…` 一起到达 Anthropic。
  - 入站 `User-Agent` 仅在以 `claude-cli/`、`claude-code/`、`anthropic-`
    或 `claude/` 开头时透传；裸 `curl/8.x`、第三方 SDK 等会被替换为配置
    默认 UA，避免触发 Anthropic 反滥用层、同时保留真正 Claude Code 流量
    在日志里的可识别性。

- **OpenAI 风格的 effort 分级**：`/v1/messages` 支持额外的顶层
  `"effort"` 字段，作为 Anthropic 的 `max_tokens` + 扩展思考
  `budget_tokens` 的别名。内置分级（可在 `upstream.effort_levels` 调整）：

  | tier   | max_tokens | thinking budget |
  | ------ | ---------- | --------------- |
  | auto   | 8192       | —               |
  | low    | 4096       | —               |
  | medium | 16384      | 5000            |
  | high   | 32768      | 15000           |
  | xhigh  | 48000      | 30000           |
  | max    | 64000      | 60000           |

  cliproxy 会在转发前剥掉 `effort` 字段（Anthropic 不识别它）。请求中已经
  显式设置的 `max_tokens` 或 `thinking` 永远优先 —— `effort` 只补默认值。
  未知 tier 返回 400 并列出可用值。

- **System prompt 自动注入**：Anthropic 的 OAuth Pro/Max 端点要求每次
  `/v1/messages` 请求的第一段 `system` 必须是字面串
  `You are Claude Code, Anthropic's official CLI for Claude.`。如果请求里
  没有（比如裸 curl、第三方 SDK），Anthropic 会返回*完全相同*的 terse
  `rate_limit_error: "Error"` —— 跟真的速率限制一模一样，常被误判。默认
  cliproxy 会在转发前把这段补到 `system` 数组的最前面，原有的 system 内容
  保留在它之后。已经以这段开头的请求（如直接来自 `claude` 本身）按字节透传，
  不会双写。要关掉这个行为，配置 `upstream.inject_claude_code_system: false`。
- **上游错误可见性**：代理会把 Anthropic 的状态码与响应体 1:1 转发，包括
  `{"type":"error","error":{...},"request_id":"..."}` 以及 `retry-after`
  / `anthropic-ratelimit-*` 头，所以未经修改的 `claude` 客户端能正常感知。
  任何 4xx/5xx，服务端日志都会打出结构化的一行（status、err_type、
  request_id、retry-after、命中的用户），同样的摘要也会写入
  `usage_events.error` 列。Anthropic 的 OAuth bearer 速率限制常常返回非常
  简短的 `"message":"Error"` —— 这是 Anthropic 自己的格式，不是 cliproxy
  改的，找官方支持时 request_id 是唯一可对账的线索。
- **模型清单**：cliproxy 启动时（之后每 30 分钟一次）会用主机的 OAuth token
  调用 Anthropic `GET /v1/models`，自动发现当前 Pro/Max 套餐能用的模型。
  Settings 页把这份清单渲染成复选框，勾选的模型会被加入全局禁用列表。如果
  之前被禁用的某个模型在最新清单中消失（Anthropic 调整了套餐，或重命名了
  模型 ID），后台会把它放进「Previously selected, no longer available」分
  区里突出显示，避免被静默忽略。点击「Refresh catalog」按钮，或调用
  `POST /admin/api/settings/refresh-models` 即可手动刷新。
- **进程托管**：用 systemd / supervisord 之类托起 cliproxy，发送 `SIGTERM`
  即可优雅关停。
- **OAuth 刷新**：access token 离过期不到 60 秒时会在进程内刷新，新的 access
  / refresh token 会以原子方式写回 Claude Code 使用的同一个凭据文件，主机
  CLI 自身依然处于登录态。
- **数据备份**：`data_dir/cliproxy.db` 保存了所有用户、Key 哈希、策略与历史
  用量。开启了 WAL 模式，备份建议用
  `sqlite3 cliproxy.db ".backup '/path/to/copy.db'"`，不要直接 cp。
- **TLS**：cliproxy 只跑 HTTP。生产环境请用 Caddy / nginx 终结 TLS，并把
  cliproxy 绑到 `127.0.0.1`。
- **重置一切**：停服 → 删除 `data_dir/cliproxy.db` → 重启即可重新初始化。

## 客户端常用接入方式

任何尊重 `ANTHROPIC_BASE_URL` 与 `ANTHROPIC_API_KEY` 的 Claude SDK / 工具都
可以直接对接。例：

```sh
# Claude Code CLI
export ANTHROPIC_BASE_URL=http://proxy.lan:8787
export ANTHROPIC_API_KEY=sk-cp-...
claude

# 直接 curl
curl http://proxy.lan:8787/v1/messages \
  -H "x-api-key: sk-cp-..." \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-5",
    "max_tokens": 256,
    "messages": [{"role":"user","content":"你好"}]
  }'
```

## 已知约束

- **Anthropic 服务条款**：把 Pro/Max 会话分给多人使用可能违反你订阅的条款，
  请自行评估合规风险。
- **默认无 TLS**：在真实网络中务必走 Caddy/nginx 终结 TLS。
- **OAuth 端点不属于公开 API**：refresh URL、`oauth_client_id`、
  `anthropic-beta` 这些值不是 Anthropic 官方公开文档。一旦上游变更，请在配
  置文件里直接覆盖默认值。
