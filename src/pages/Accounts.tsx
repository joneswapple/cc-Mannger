import { useState } from "react";
import { get, openExternal, post } from "../api";
import { fmtDuration, fmtRelative, fmtTime } from "../format";
import type { Account, AccountsResponse } from "../types";
import { Badge, Button, Card, ErrorBox, Field, Meter, Modal, inputClass, useAction, useLoad } from "../ui";

export function statusBadge(a: Account, now: number) {
  if (!a.enabled) return <Badge>已禁用</Badge>;
  if (a.status === "needs_reauth") return <Badge tone="red">需重新登录</Badge>;
  if (a.cooldown_until > now) return <Badge tone="yellow">冷却 {fmtDuration(a.cooldown_until - now)}</Badge>;
  return <Badge tone="green">正常</Badge>;
}

export default function Accounts() {
  const { data, error, reload } = useLoad(() => get<AccountsResponse>("/accounts"));
  const [view, setView] = useState<"cards" | "table">("cards");
  const [modal, setModal] = useState<"login" | "import" | null>(null);
  const [editing, setEditing] = useState<Account | null>(null);
  const act = useAction();

  const run = async (fn: () => Promise<unknown>) => {
    await act.run(fn);
    await reload();
  };

  const accounts = data?.accounts ?? [];
  const now = data?.now ?? Date.now() / 1000;

  return (
    <div className="space-y-4">
      <Card
        title={`上游账号（${accounts.length}）`}
        actions={
          <>
            <Button tone="ghost" onClick={() => setView(view === "cards" ? "table" : "cards")}>
              {view === "cards" ? "列表视图" : "卡片视图"}
            </Button>
            <Button onClick={() => setModal("import")}>导入凭据</Button>
            <Button tone="primary" onClick={() => setModal("login")}>
              OAuth 登录
            </Button>
          </>
        }
      >
        <ErrorBox error={error ?? act.error} />
        {data && (
          <p className="mb-3 text-xs text-slate-500">
            调度策略 <code>{data.pool.strategy}</code> · 粘性会话 {data.pool.sticky ? "开" : "关"} · 失败最多再试 {data.pool.max_retries} 个账号
          </p>
        )}
        {accounts.length === 0 && !error && <p className="text-sm text-slate-500">还没有账号。用 OAuth 登录或导入 Claude Code 凭据添加。</p>}
        {view === "cards" ? (
          <div className="grid gap-3 md:grid-cols-2">
            {accounts.map((a) => (
              <div key={a.id} className="rounded-md border border-slate-200 p-3 dark:border-slate-800">
                <div className="flex items-start justify-between gap-2">
                  <div className="min-w-0">
                    <div className="truncate font-medium">
                      #{a.id} {a.label}
                    </div>
                    <div className="truncate text-xs text-slate-500">
                      {a.email || "（未知邮箱）"} {a.plan && `· ${a.plan}`}
                    </div>
                  </div>
                  {statusBadge(a, now)}
                </div>
                <div className="mt-2 flex flex-wrap gap-1 text-xs text-slate-500">
                  <Badge>优先级 {a.priority}</Badge>
                  <Badge>权重 {a.weight}</Badge>
                  {a.account_group && <Badge tone="blue">组 {a.account_group}</Badge>}
                  {a.proxy_url && <Badge>代理</Badge>}
                  {a.sync_file && <Badge>同步 CLI 凭据</Badge>}
                </div>
                {a.quota && (
                  <div className="mt-2 grid grid-cols-2 gap-2">
                    <Meter label={`5 小时 · ${fmtRelative(a.quota.five_hour_reset, now)}重置`} value={a.quota.five_hour_utilization} />
                    <Meter label={`7 天 · ${fmtRelative(a.quota.seven_day_reset, now)}重置`} value={a.quota.seven_day_utilization} />
                  </div>
                )}
                {a.status_reason && <p className="mt-2 break-words text-xs text-slate-500">{a.status_reason}</p>}
                <AccountActions a={a} now={now} busy={act.busy} run={run} onEdit={() => setEditing(a)} />
              </div>
            ))}
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm">
              <thead className="text-xs text-slate-500">
                <tr>
                  <th className="py-1">账号</th>
                  <th>状态</th>
                  <th>优先级 / 权重</th>
                  <th>组</th>
                  <th>5h / 7d</th>
                  <th>更新</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {accounts.map((a) => (
                  <tr key={a.id} className="border-t border-slate-100 dark:border-slate-800">
                    <td className="py-2">
                      #{a.id} {a.label}
                      <div className="text-xs text-slate-500">{a.email}</div>
                    </td>
                    <td>{statusBadge(a, now)}</td>
                    <td>
                      {a.priority} / {a.weight}
                    </td>
                    <td>{a.account_group || "—"}</td>
                    <td className="text-xs">
                      {a.quota ? `${pct(a.quota.five_hour_utilization)} / ${pct(a.quota.seven_day_utilization)}` : "—"}
                    </td>
                    <td className="text-xs">{fmtTime(a.updated_at)}</td>
                    <td>
                      <AccountActions a={a} now={now} busy={act.busy} run={run} onEdit={() => setEditing(a)} compact />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
      {modal === "login" && <LoginModal onClose={() => setModal(null)} onDone={reload} />}
      {modal === "import" && <ImportModal onClose={() => setModal(null)} onDone={reload} />}
      {editing && <EditModal account={editing} onClose={() => setEditing(null)} onDone={reload} />}
    </div>
  );
}

const pct = (v: number | null) => (v === null ? "—" : `${Math.round(v * 100)}%`);

function AccountActions({
  a,
  now,
  busy,
  run,
  onEdit,
  compact,
}: {
  a: Account;
  now: number;
  busy: boolean;
  run: (fn: () => Promise<unknown>) => Promise<void>;
  onEdit: () => void;
  compact?: boolean;
}) {
  return (
    <div className={`flex flex-wrap gap-1 ${compact ? "" : "mt-3"}`}>
      <Button small disabled={busy} onClick={onEdit}>
        编辑
      </Button>
      <Button small disabled={busy} onClick={() => run(() => post(`/accounts/${a.id}/pin`))}>
        置顶
      </Button>
      <Button small disabled={busy} onClick={() => run(() => post(`/accounts/${a.id}/refresh`))}>
        刷新
      </Button>
      <Button small disabled={busy} onClick={() => run(() => post(`/accounts/${a.id}`, { enabled: !a.enabled }))}>
        {a.enabled ? "禁用" : "启用"}
      </Button>
      {a.cooldown_until > now && (
        <Button small disabled={busy} onClick={() => run(() => post(`/accounts/${a.id}`, { clear_cooldown: true }))}>
          解除冷却
        </Button>
      )}
      <Button
        small
        tone="danger"
        disabled={busy}
        onClick={() => {
          if (confirm(`删除账号 #${a.id} ${a.label}？保存的 token 会一并删除。`)) void run(() => post(`/accounts/${a.id}/delete`));
        }}
      >
        删除
      </Button>
    </div>
  );
}

function LoginModal({ onClose, onDone }: { onClose: () => void; onDone: () => void }) {
  const [label, setLabel] = useState("");
  const [group, setGroup] = useState("");
  const [start, setStart] = useState<{ url: string; state: string } | null>(null);
  const [code, setCode] = useState("");
  const [done, setDone] = useState<Account | null>(null);
  const act = useAction();
  return (
    <Modal title="OAuth 登录 Claude 账号" onClose={onClose}>
      <ErrorBox error={act.error} />
      {done ? (
        <>
          <p className="text-sm">
            已添加 #{done.id} {done.label}（{done.email || "未知邮箱"}）。
          </p>
          <Button tone="primary" onClick={onClose}>
            完成
          </Button>
        </>
      ) : !start ? (
        <>
          <Field label="标签（可选）" hint="不填则用账号邮箱">
            <input className={inputClass} value={label} onChange={(e) => setLabel(e.target.value)} />
          </Field>
          <Field label="账号组（可选）">
            <input className={inputClass} value={group} onChange={(e) => setGroup(e.target.value)} />
          </Field>
          <Button
            tone="primary"
            disabled={act.busy}
            onClick={async () => {
              const s = await act.run(() => post<{ url: string; state: string }>("/accounts/oauth/start", { label, account_group: group }));
              if (s) {
                setStart(s);
                void openExternal(s.url);
              }
            }}
          >
            生成授权链接
          </Button>
        </>
      ) : (
        <>
          <p className="text-sm">1. 在已登录目标 Claude 账号的浏览器里打开授权链接并同意：</p>
          <div className="flex gap-2">
            <input className={`${inputClass} font-mono text-xs`} readOnly value={start.url} onFocus={(e) => e.target.select()} />
            <Button small onClick={() => openExternal(start.url)}>
              打开
            </Button>
            <Button small onClick={() => navigator.clipboard.writeText(start.url).catch(() => {})}>
              复制
            </Button>
          </div>
          <p className="text-sm">2. 把页面上显示的授权码（形如 <code>code#state</code>）粘贴到这里：</p>
          <input className={`${inputClass} font-mono`} value={code} onChange={(e) => setCode(e.target.value)} placeholder="code#state" />
          <Button
            tone="primary"
            disabled={act.busy || !code.trim()}
            onClick={async () => {
              const a = await act.run(() => post<Account>("/accounts/oauth/complete", { code: code.trim(), state: start.state }));
              if (a) {
                setDone(a);
                onDone();
              }
            }}
          >
            完成登录
          </Button>
        </>
      )}
    </Modal>
  );
}

function ImportModal({ onClose, onDone }: { onClose: () => void; onDone: () => void }) {
  const [text, setText] = useState("");
  const [label, setLabel] = useState("");
  const [group, setGroup] = useState("");
  const [result, setResult] = useState<{ imported: Account[]; errors: { index: number; error: string }[] } | null>(null);
  const act = useAction();
  return (
    <Modal title="导入凭据" onClose={onClose}>
      <p className="text-xs text-slate-500">
        粘贴 Claude Code 的 <code>~/.claude/.credentials.json</code> 内容，或多个凭据组成的 JSON 数组（元素可以是凭据本身，或{" "}
        <code>{`{"label","account_group","credentials"}`}</code>）。邮箱相同的账号会更新而不是重复添加。
      </p>
      <ErrorBox error={act.error} />
      <textarea className={`${inputClass} h-40 font-mono text-xs`} value={text} onChange={(e) => setText(e.target.value)} />
      <input
        type="file"
        accept=".json,application/json"
        className="text-xs"
        onChange={async (e) => {
          const f = e.target.files?.[0];
          if (f) setText(await f.text());
        }}
      />
      <div className="grid grid-cols-2 gap-2">
        <Field label="标签（可选）">
          <input className={inputClass} value={label} onChange={(e) => setLabel(e.target.value)} />
        </Field>
        <Field label="账号组（可选）">
          <input className={inputClass} value={group} onChange={(e) => setGroup(e.target.value)} />
        </Field>
      </div>
      {result && (
        <div className="text-sm">
          导入成功 {result.imported.length} 个
          {result.errors.map((e) => (
            <div key={e.index} className="text-red-600">
              第 {e.index + 1} 项：{e.error}
            </div>
          ))}
        </div>
      )}
      <Button
        tone="primary"
        disabled={act.busy || !text.trim()}
        onClick={async () => {
          const r = await act.run(async () => {
            let credentials: unknown;
            try {
              credentials = JSON.parse(text);
            } catch {
              throw new Error("不是有效的 JSON");
            }
            return post<{ imported: Account[]; errors: { index: number; error: string }[] }>("/accounts/import", {
              credentials,
              label,
              account_group: group,
            });
          });
          if (r) {
            setResult(r);
            onDone();
          }
        }}
      >
        导入
      </Button>
    </Modal>
  );
}

function EditModal({ account, onClose, onDone }: { account: Account; onClose: () => void; onDone: () => void }) {
  const [f, setF] = useState({
    label: account.label,
    priority: String(account.priority),
    weight: String(account.weight),
    account_group: account.account_group,
    proxy_url: account.proxy_url,
  });
  const act = useAction();
  const set = (k: keyof typeof f) => (e: React.ChangeEvent<HTMLInputElement>) => setF({ ...f, [k]: e.target.value });
  return (
    <Modal title={`编辑账号 #${account.id}`} onClose={onClose}>
      <ErrorBox error={act.error} />
      <Field label="标签">
        <input className={inputClass} value={f.label} onChange={set("label")} />
      </Field>
      <div className="grid grid-cols-2 gap-2">
        <Field label="优先级" hint="priority 策略下越大越优先">
          <input className={inputClass} type="number" value={f.priority} onChange={set("priority")} />
        </Field>
        <Field label="权重" hint="round_robin 策略下的分配比例">
          <input className={inputClass} type="number" min={1} value={f.weight} onChange={set("weight")} />
        </Field>
      </div>
      <Field label="账号组" hint="绑定到该组的终端只会用组内账号">
        <input className={inputClass} value={f.account_group} onChange={set("account_group")} />
      </Field>
      <Field label="出站代理" hint="http://、https://、socks5:// 或 socks5h://；留空直连。设置后该账号的所有请求都只走代理">
        <input className={`${inputClass} font-mono`} value={f.proxy_url} onChange={set("proxy_url")} placeholder="socks5://127.0.0.1:1080" />
      </Field>
      <Button
        tone="primary"
        disabled={act.busy}
        onClick={async () => {
          const r = await act.run(() =>
            post(`/accounts/${account.id}`, {
              label: f.label,
              priority: Number(f.priority) || 0,
              weight: Math.max(1, Number(f.weight) || 1),
              account_group: f.account_group,
              proxy_url: f.proxy_url,
            }),
          );
          if (r !== undefined) {
            onDone();
            onClose();
          }
        }}
      >
        保存
      </Button>
    </Modal>
  );
}
