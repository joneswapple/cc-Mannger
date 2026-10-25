import { useState } from "react";
import { get, post } from "../api";
import { fmtTime } from "../format";
import type { Invite } from "../types";
import { Badge, Button, Card, ErrorBox, Field, SecretOnce, inputClass, useAction, useLoad } from "../ui";

export default function Invites() {
  const { data, error, reload } = useLoad(() => get<{ invites: Invite[] }>("/invites"));
  const [label, setLabel] = useState("");
  const [ttl, setTtl] = useState("24");
  const [token, setToken] = useState<string | null>(null);
  const act = useAction();
  const now = Date.now() / 1000;

  return (
    <div className="space-y-4">
      <Card title="创建邀请码">
        <ErrorBox error={act.error} />
        <div className="flex flex-wrap items-end gap-2">
          <div className="min-w-48 flex-1">
            <Field label="标签">
              <input className={inputClass} value={label} onChange={(e) => setLabel(e.target.value)} placeholder="bob-desktop" />
            </Field>
          </div>
          <div className="w-28">
            <Field label="有效期（小时）">
              <input className={inputClass} type="number" min={1} value={ttl} onChange={(e) => setTtl(e.target.value)} />
            </Field>
          </div>
          <Button
            tone="primary"
            disabled={act.busy || !label.trim()}
            onClick={async () => {
              const r = await act.run(() => post<{ token: string }>("/invites", { label, ttl_hours: Number(ttl) || 24 }));
              if (r) {
                setToken(r.token);
                setLabel("");
                await reload();
              }
            }}
          >
            创建
          </Button>
        </div>
        {token && (
          <div className="mt-3 space-y-2">
            <SecretOnce label="邀请码" value={token} />
            <p className="text-xs text-slate-500">
              终端用它换 API Key：
              <code className="break-all">
                {`curl -X POST ${location.origin}/register -H "Content-Type: application/json" -d '{"token":"${token}","label":"my-laptop"}'`}
              </code>
            </p>
          </div>
        )}
      </Card>
      <Card title="邀请码">
        <ErrorBox error={error} />
        <table className="w-full text-left text-sm">
          <thead className="text-xs text-slate-500">
            <tr>
              <th className="py-1">标签</th>
              <th>创建</th>
              <th>过期</th>
              <th>状态</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {data?.invites.map((i) => (
              <tr key={i.token_hash} className="border-t border-slate-100 dark:border-slate-800">
                <td className="py-2">{i.label}</td>
                <td className="text-xs">{fmtTime(i.created_at)}</td>
                <td className="text-xs">{fmtTime(i.expires_at)}</td>
                <td>
                  {i.redeemed_at ? (
                    <Badge tone="blue">已使用（用户 #{i.user_id}）</Badge>
                  ) : i.expires_at < now ? (
                    <Badge>已过期</Badge>
                  ) : (
                    <Badge tone="green">可用</Badge>
                  )}
                </td>
                <td className="text-right">
                  {!i.redeemed_at && (
                    <Button
                      small
                      tone="danger"
                      disabled={act.busy}
                      onClick={async () => {
                        await act.run(() => post(`/invites/${i.token_hash}/delete`));
                        await reload();
                      }}
                    >
                      删除
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
    </div>
  );
}
