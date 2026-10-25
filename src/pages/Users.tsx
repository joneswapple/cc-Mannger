import { useState } from "react";
import { get, post } from "../api";
import { fmtRelative, fmtTime } from "../format";
import type { Policy, Settings, User } from "../types";
import { Badge, Button, Card, ErrorBox, Field, Modal, SecretOnce, inputClass, useAction, useLoad } from "../ui";

export default function Users() {
  const { data, error, reload } = useLoad(() => get<{ users: User[] }>("/users"));
  const [secret, setSecret] = useState<{ label: string; value: string } | null>(null);
  const [creating, setCreating] = useState(false);
  const [policyFor, setPolicyFor] = useState<User | null>(null);
  const act = useAction();

  const run = async (fn: () => Promise<unknown>) => {
    await act.run(fn);
    await reload();
  };

  const issueKey = async (u: User) => {
    const r = await act.run(() => post<{ api_key: string }>("/keys/issue", { user_id: u.id }));
    if (r) setSecret({ label: `${u.label} 的新 API Key`, value: r.api_key });
    await reload();
  };

  return (
    <div className="space-y-4">
      {secret && (
        <div className="space-y-2">
          <SecretOnce label={secret.label} value={secret.value} />
          <Button small tone="ghost" onClick={() => setSecret(null)}>
            我已保存
          </Button>
        </div>
      )}
      <Card
        title={`终端用户（${data?.users.length ?? 0}）`}
        actions={
          <Button tone="primary" onClick={() => setCreating(true)}>
            新建用户
          </Button>
        }
      >
        <ErrorBox error={error ?? act.error} />
        <div className="space-y-3">
          {data?.users.map((u) => (
            <div key={u.id} className="rounded-md border border-slate-200 p-3 dark:border-slate-800">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <span className="font-medium">
                    #{u.id} {u.label}
                  </span>{" "}
                  {u.enabled ? <Badge tone="green">启用</Badge> : <Badge tone="red">禁用</Badge>}
                  <span className="ml-2 text-xs text-slate-500">创建于 {fmtTime(u.created_at)}</span>
                </div>
                <div className="flex flex-wrap gap-1">
                  <Button small onClick={() => setPolicyFor(u)}>
                    策略
                  </Button>
                  <Button small onClick={() => issueKey(u)} disabled={act.busy}>
                    签发 Key
                  </Button>
                  <Button small disabled={act.busy} onClick={() => run(() => post(`/users/${u.id}/${u.enabled ? "disable" : "enable"}`))}>
                    {u.enabled ? "禁用" : "启用"}
                  </Button>
                </div>
              </div>
              <PolicySummary p={u.policy} />
              {u.keys.length > 0 && (
                <table className="mt-2 w-full text-left text-xs">
                  <thead className="text-slate-500">
                    <tr>
                      <th>Key</th>
                      <th>创建</th>
                      <th>最近使用</th>
                      <th>状态</th>
                      <th />
                    </tr>
                  </thead>
                  <tbody>
                    {u.keys.map((k) => (
                      <tr key={k.id} className="border-t border-slate-100 dark:border-slate-800">
                        <td className="py-1 font-mono">{k.prefix}…</td>
                        <td>{fmtTime(k.created_at)}</td>
                        <td>{fmtRelative(k.last_used_at)}</td>
                        <td>{k.revoked_at ? <Badge tone="red">已吊销</Badge> : <Badge tone="green">有效</Badge>}</td>
                        <td className="text-right">
                          {!k.revoked_at && (
                            <Button
                              small
                              tone="danger"
                              disabled={act.busy}
                              onClick={() => confirm(`吊销 ${k.prefix}…？`) && run(() => post(`/keys/${k.id}/revoke`))}
                            >
                              吊销
                            </Button>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          ))}
        </div>
      </Card>
      {creating && (
        <CreateUserModal
          onClose={() => setCreating(false)}
          onCreated={async (label, key) => {
            setSecret({ label: `${label} 的 API Key`, value: key });
            setCreating(false);
            await reload();
          }}
        />
      )}
      {policyFor && <PolicyModal user={policyFor} onClose={() => setPolicyFor(null)} onDone={reload} />}
    </div>
  );
}

function PolicySummary({ p }: { p: Policy }) {
  const parts = [
    p.allowed_models ? `模型白名单：${p.allowed_models.join(", ") || "（空）"}` : null,
    p.rpm_limit !== null ? `每分钟 ${p.rpm_limit} 次` : null,
    p.daily_token_limit !== null ? `每日 ${p.daily_token_limit.toLocaleString()} tokens` : null,
    p.account_group ? `账号组 ${p.account_group}` : null,
  ].filter(Boolean);
  if (!parts.length) return <p className="mt-1 text-xs text-slate-500">无限制</p>;
  return <p className="mt-1 text-xs text-slate-500">{parts.join(" · ")}</p>;
}

function CreateUserModal({ onClose, onCreated }: { onClose: () => void; onCreated: (label: string, key: string) => void }) {
  const [label, setLabel] = useState("");
  const act = useAction();
  return (
    <Modal title="新建终端用户" onClose={onClose}>
      <ErrorBox error={act.error} />
      <Field label="标签" hint="例如 alice-laptop；创建后会立即签发一个 API Key">
        <input className={inputClass} value={label} onChange={(e) => setLabel(e.target.value)} autoFocus />
      </Field>
      <Button
        tone="primary"
        disabled={act.busy || !label.trim()}
        onClick={async () => {
          const r = await act.run(async () => {
            const u = await post<{ user_id: number }>("/users", { label });
            return post<{ api_key: string }>("/keys/issue", { user_id: u.user_id });
          });
          if (r) onCreated(label, r.api_key);
        }}
      >
        创建
      </Button>
    </Modal>
  );
}

function PolicyModal({ user, onClose, onDone }: { user: User; onClose: () => void; onDone: () => void }) {
  const p = user.policy;
  const settings = useLoad(() => get<Settings>("/settings"));
  const [restrict, setRestrict] = useState(p.allowed_models !== null);
  const [models, setModels] = useState<string[]>(p.allowed_models ?? []);
  const [rpm, setRpm] = useState(p.rpm_limit?.toString() ?? "");
  const [daily, setDaily] = useState(p.daily_token_limit?.toString() ?? "");
  const [group, setGroup] = useState(p.account_group ?? "");
  const act = useAction();
  const num = (s: string) => (s.trim() === "" ? null : Number(s));
  const known = settings.data?.models.map((m) => m.id) ?? [];
  const all = Array.from(new Set([...known, ...models]));
  return (
    <Modal title={`策略：${user.label}`} onClose={onClose}>
      <ErrorBox error={act.error} />
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" checked={restrict} onChange={(e) => setRestrict(e.target.checked)} />
        只允许以下模型
      </label>
      {restrict && (
        <div className="max-h-40 space-y-1 overflow-y-auto rounded border border-slate-200 p-2 text-sm dark:border-slate-800">
          {all.length === 0 && <span className="text-xs text-slate-500">模型目录为空</span>}
          {all.map((m) => (
            <label key={m} className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={models.includes(m)}
                onChange={(e) => setModels(e.target.checked ? [...models, m] : models.filter((x) => x !== m))}
              />
              <code className="text-xs">{m}</code>
            </label>
          ))}
        </div>
      )}
      <div className="grid grid-cols-2 gap-2">
        <Field label="每分钟请求数" hint="留空不限">
          <input className={inputClass} type="number" min={1} value={rpm} onChange={(e) => setRpm(e.target.value)} />
        </Field>
        <Field label="每日 token 额度" hint="留空不限">
          <input className={inputClass} type="number" min={1} value={daily} onChange={(e) => setDaily(e.target.value)} />
        </Field>
      </div>
      <Field label="账号组" hint="只使用该组的上游账号；留空则所有账号可用">
        <input className={inputClass} value={group} onChange={(e) => setGroup(e.target.value)} />
      </Field>
      <Button
        tone="primary"
        disabled={act.busy}
        onClick={async () => {
          const r = await act.run(() =>
            post(`/users/${user.id}/policy`, {
              allowed_models: restrict ? models : null,
              rpm_limit: num(rpm),
              daily_token_limit: num(daily),
              account_group: group.trim() || null,
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
