import { BarChart3, Ban, CalendarPlus, ChevronDown, ChevronRight, KeyRound, Monitor, Plus, Power, RefreshCw, Search, ShieldCheck, Ticket, Trash2 } from "lucide-react";
import { useState } from "react";
import { get, post } from "../api";
import { fmtNum, fmtRelative, fmtTime } from "../format";
import { useI18n } from "../i18n";
import { useToast } from "../toast";
import type { Invite, KeyInfo, Policy, Settings, User, UserIp } from "../types";
import { Badge, Button, Card, ErrorBox, Field, IconButton, Modal, SecretOnce, Segmented, StatCard, inputBase, inputClass, useAction, useLoad } from "../ui";
import StatsOverview from "./StatsOverview";

type Tab = "users" | "invites";

export default function Terminals() {
  const { t } = useI18n();
  const users = useLoad(() => get<{ users: User[] }>(`/users?tz_offset=${-new Date().getTimezoneOffset()}`));
  const invites = useLoad(() => get<{ invites: Invite[] }>("/invites"));
  const [tab, setTab] = useState<Tab>("users");
  const now = Date.now() / 1000;

  const list = users.data?.users ?? [];
  const activeKeys = list.reduce((n, u) => n + u.keys.filter((k) => !k.revoked_at).length, 0);
  const pending = (invites.data?.invites ?? []).filter((i) => !i.redeemed_at && i.expires_at > now).length;

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h1 className="text-2xl font-bold tracking-tight">{t("nav.terminals")}</h1>
        <Button
          icon={<RefreshCw className={`h-4 w-4 ${users.loading ? "animate-spin" : ""}`} />}
          onClick={() => {
            void users.reload();
            void invites.reload();
          }}
        >
          {t("common.refresh")}
        </Button>
      </div>

      <div className="grid gap-4 sm:grid-cols-3">
        <StatCard
          icon={<Monitor className="h-5 w-5 text-blue-500" />}
          iconBg="bg-blue-50 dark:bg-blue-950"
          value={list.length}
          label={t("terminals.total")}
          hint={t("terminals.enabled_n", { n: list.filter((u) => u.enabled).length })}
        />
        <StatCard icon={<KeyRound className="h-5 w-5 text-emerald-500" />} iconBg="bg-emerald-50 dark:bg-emerald-950" value={activeKeys} label={t("terminals.active_keys")} />
        <StatCard icon={<Ticket className="h-5 w-5 text-violet-500" />} iconBg="bg-violet-50 dark:bg-violet-950" value={pending} label={t("terminals.pending_invites")} />
      </div>

      <Segmented
        value={tab}
        onChange={setTab}
        options={[
          { key: "users", label: t("terminals.tab_users") },
          { key: "invites", label: t("terminals.tab_invites") },
        ]}
      />

      {tab === "users" ? <UsersTab users={list} error={users.error} reload={users.reload} /> : <InvitesTab invites={invites.data?.invites ?? []} error={invites.error} reload={invites.reload} />}
    </div>
  );
}

function UsersTab({ users, error, reload }: { users: User[]; error: string | null; reload: () => Promise<void> }) {
  const { t } = useI18n();
  const toast = useToast();
  const [q, setQ] = useState("");
  const [open, setOpen] = useState<Set<number>>(new Set());
  const [secret, setSecret] = useState<{ label: string; value: string } | null>(null);
  const [creating, setCreating] = useState(false);
  const [policyFor, setPolicyFor] = useState<User | null>(null);
  const [issueFor, setIssueFor] = useState<User | null>(null);
  const [renew, setRenew] = useState<KeyInfo | null>(null);
  const [detailFor, setDetailFor] = useState<User | null>(null);
  const act = useAction();

  const run = async (fn: () => Promise<unknown>, ok?: string) => {
    const r = await act.run(fn);
    if (r !== undefined && ok) toast("success", ok);
    await reload();
  };


  const toggleOpen = (id: number) => {
    const next = new Set(open);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setOpen(next);
  };

  const shown = users.filter((u) => !q.trim() || u.label.toLowerCase().includes(q.trim().toLowerCase()) || String(u.id) === q.trim());

  return (
    <div className="space-y-4">
      {secret && (
        <div className="space-y-2">
          <SecretOnce label={secret.label} value={secret.value} />
          <Button small tone="ghost" onClick={() => setSecret(null)}>
            {t("terminals.saved_it")}
          </Button>
        </div>
      )}
      <Card bodyClass="p-0">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-100 p-4 dark:border-slate-800">
          <div className="relative min-w-48 flex-1 sm:max-w-xs">
            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-400" />
            <input className={`${inputBase} w-full pl-9`} placeholder={t("terminals.search")} value={q} onChange={(e) => setQ(e.target.value)} />
          </div>
          <Button tone="primary" icon={<Plus className="h-4 w-4" />} onClick={() => setCreating(true)}>
            {t("terminals.new_user")}
          </Button>
        </div>
        <div className="px-4">
          <ErrorBox error={error ?? act.error} />
        </div>
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead className="bg-slate-50 text-xs uppercase tracking-wide text-slate-500 dark:bg-slate-950/50">
              <tr>
                <th className="w-8 py-3 pl-4" />
                <th className="py-3">{t("terminals.label")}</th>
                <th>{t("terminals.status")}</th>
                <th>{t("terminals.policy")}</th>
                <th>{t("terminals.keys")}</th>
                <th>{t("terminals.today")}</th>
                <th>{t("terminals.created")}</th>
                <th className="pr-4 text-right">{t("common.actions")}</th>
              </tr>
            </thead>
            <tbody>
              {shown.map((u) => {
                const live = u.keys.filter((k) => !k.revoked_at);
                const lastUsed = Math.max(0, ...u.keys.map((k) => k.last_used_at ?? 0));
                return [
                  <tr key={u.id} className="border-t border-slate-100 hover:bg-slate-50/60 dark:border-slate-800 dark:hover:bg-slate-800/30">
                    <td className="py-3 pl-4">
                      <IconButton title={t("terminals.keys")} onClick={() => toggleOpen(u.id)} disabled={u.keys.length === 0}>
                        {open.has(u.id) ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
                      </IconButton>
                    </td>
                    <td className="py-3">
                      <div className="font-medium">{u.label}</div>
                      <div className="text-xs text-slate-400">
                        #{u.id} · {t("terminals.last_used")} {lastUsed ? fmtRelative(lastUsed) : t("common.never")}
                      </div>
                    </td>
                    <td>{u.enabled ? <Badge tone="green">{t("common.enabled")}</Badge> : <Badge tone="red">{t("common.disabled")}</Badge>}</td>
                    <td className="max-w-xs">
                      <PolicySummary p={u.policy} />
                    </td>
                    <td>
                      <span className="font-medium">{live.length}</span>
                      <span className="text-slate-400"> / {u.keys.length}</span>
                    </td>
                    <td className="whitespace-nowrap text-xs">
                      <span className="font-medium">{t("usage.times", { n: u.today_requests })}</span>
                      <span className="text-slate-400"> · {fmtNum(u.today_tokens)}</span>
                    </td>
                    <td className="whitespace-nowrap text-xs text-slate-500">{fmtTime(u.created_at)}</td>
                    <td className="pr-4">
                      <div className="flex justify-end gap-0.5">
                        <IconButton title={t("terminals.usage")} onClick={() => setDetailFor(u)}>
                          <BarChart3 className="h-4 w-4" />
                        </IconButton>
                        <IconButton title={t("terminals.policy")} onClick={() => setPolicyFor(u)}>
                          <ShieldCheck className="h-4 w-4" />
                        </IconButton>
                        <IconButton title={t("terminals.issue_key")} disabled={act.busy} onClick={() => setIssueFor(u)}>
                          <KeyRound className="h-4 w-4" />
                        </IconButton>
                        <IconButton
                          title={u.enabled ? t("common.disable") : t("common.enable")}
                          disabled={act.busy}
                          onClick={() => run(() => post(`/users/${u.id}/${u.enabled ? "disable" : "enable"}`), t("common.done"))}
                        >
                          <Power className={`h-4 w-4 ${u.enabled ? "text-emerald-500" : ""}`} />
                        </IconButton>
                      </div>
                    </td>
                  </tr>,
                  open.has(u.id) && (
                    <tr key={`${u.id}-keys`} className="bg-slate-50/60 dark:bg-slate-950/40">
                      <td />
                      <td colSpan={7} className="py-2 pr-4">
                        <table className="w-full text-left text-xs">
                          <thead className="text-slate-400">
                            <tr>
                              <th className="py-1">{t("terminals.key")}</th>
                              <th>{t("terminals.created")}</th>
                              <th>{t("terminals.last_used")}</th>
                              <th>{t("terminals.expires")}</th>
                              <th>{t("terminals.status")}</th>
                              <th />
                            </tr>
                          </thead>
                          <tbody>
                            {u.keys.map((k) => (
                              <tr key={k.id} className="border-t border-slate-100 dark:border-slate-800">
                                <td className="py-1.5 font-mono">{k.prefix}…</td>
                                <td>{fmtTime(k.created_at)}</td>
                                <td>{k.last_used_at ? fmtRelative(k.last_used_at) : t("common.never")}</td>
                                <td>{k.expires_at ? fmtTime(k.expires_at) : t("terminals.never_expires")}</td>
                                <td>
                                  <KeyStatus k={k} />
                                </td>
                                <td className="text-right">
                                  {!k.revoked_at && (
                                    <IconButton title={t("terminals.renew")} disabled={act.busy} onClick={() => setRenew(k)}>
                                      <CalendarPlus className="h-4 w-4" />
                                    </IconButton>
                                  )}
                                  {!k.revoked_at && (
                                    <IconButton
                                      danger
                                      title={t("terminals.revoke")}
                                      disabled={act.busy}
                                      onClick={() => confirm(t("terminals.revoke_confirm", { p: k.prefix })) && run(() => post(`/keys/${k.id}/revoke`), t("terminals.revoked"))}
                                    >
                                      <Ban className="h-4 w-4" />
                                    </IconButton>
                                  )}
                                </td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      </td>
                    </tr>
                  ),
                ];
              })}
              {shown.length === 0 && (
                <tr>
                  <td colSpan={8} className="py-10 text-center text-sm text-slate-400">
                    {t("terminals.empty")}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </Card>
      {creating && (
        <CreateUserModal
          onClose={() => setCreating(false)}
          onCreated={async (label, key) => {
            setSecret({ label: t("terminals.new_key", { name: label }), value: key });
            setCreating(false);
            await reload();
          }}
        />
      )}
      {policyFor && <PolicyModal user={policyFor} onClose={() => setPolicyFor(null)} onDone={reload} />}
      {issueFor && (
        <IssueKeyModal
          user={issueFor}
          onClose={() => setIssueFor(null)}
          onIssued={async (key) => {
            setSecret({ label: t("terminals.new_key", { name: issueFor.label }), value: key });
            setIssueFor(null);
            await reload();
          }}
        />
      )}
      {renew && <RenewModal k={renew} onClose={() => setRenew(null)} onDone={reload} />}
      {detailFor && <TerminalDetail user={detailFor} onClose={() => setDetailFor(null)} />}
    </div>
  );
}

function KeyStatus({ k }: { k: KeyInfo }) {
  const { t } = useI18n();
  const now = Date.now() / 1000;
  if (k.revoked_at) return <Badge tone="red">{t("terminals.revoked")}</Badge>;
  if (k.expires_at && k.expires_at <= now) return <Badge tone="red">{t("terminals.expired_key")}</Badge>;
  if (k.expires_at && k.expires_at - now < 3 * 86400) return <Badge tone="yellow">{t("terminals.expiring", { t: fmtRelative(k.expires_at) })}</Badge>;
  return <Badge tone="green">{t("terminals.valid")}</Badge>;
}

const TTLS = [0, 7, 30, 90, 365];

function TtlSelect({ value, onChange }: { value: number; onChange: (v: number) => void }) {
  const { t } = useI18n();
  return (
    <select className={inputClass} value={value} onChange={(e) => onChange(Number(e.target.value))}>
      {TTLS.map((d) => (
        <option key={d} value={d}>
          {d === 0 ? t("terminals.never_expires") : t("terminals.ttl_days", { n: d })}
        </option>
      ))}
    </select>
  );
}

function IssueKeyModal({ user, onClose, onIssued }: { user: User; onClose: () => void; onIssued: (key: string) => void }) {
  const { t } = useI18n();
  const [ttl, setTtl] = useState(0);
  const act = useAction();
  return (
    <Modal title={`${t("terminals.issue_key")} · ${user.label}`} onClose={onClose}>
      <ErrorBox error={act.error} />
      <Field label={t("terminals.ttl")} hint={t("terminals.ttl_hint")}>
        <TtlSelect value={ttl} onChange={setTtl} />
      </Field>
      <div className="flex justify-end gap-2">
        <Button onClick={onClose}>{t("common.cancel")}</Button>
        <Button
          tone="primary"
          disabled={act.busy}
          onClick={async () => {
            const r = await act.run(() => post<{ api_key: string }>("/keys/issue", { user_id: user.id, ttl_days: ttl }));
            if (r) onIssued(r.api_key);
          }}
        >
          {t("terminals.issue_key")}
        </Button>
      </div>
    </Modal>
  );
}

function RenewModal({ k, onClose, onDone }: { k: KeyInfo; onClose: () => void; onDone: () => Promise<void> }) {
  const { t } = useI18n();
  const toast = useToast();
  const [ttl, setTtl] = useState(30);
  const act = useAction();
  return (
    <Modal title={`${t("terminals.renew")} · ${k.prefix}…`} onClose={onClose}>
      <ErrorBox error={act.error} />
      <p className="text-sm text-slate-500">
        {t("terminals.current_expiry")}: {k.expires_at ? fmtTime(k.expires_at) : t("terminals.never_expires")}
      </p>
      <Field label={t("terminals.renew_to")}>
        <TtlSelect value={ttl} onChange={setTtl} />
      </Field>
      <div className="flex justify-end gap-2">
        <Button onClick={onClose}>{t("common.cancel")}</Button>
        <Button
          tone="primary"
          disabled={act.busy}
          onClick={async () => {
            const r = await act.run(() => post(`/keys/${k.id}/expiry`, { days: ttl }));
            if (r !== undefined) {
              toast("success", t("common.saved"));
              await onDone();
              onClose();
            }
          }}
        >
          {t("common.save")}
        </Button>
      </div>
    </Modal>
  );
}

function TerminalDetail({ user, onClose }: { user: User; onClose: () => void }) {
  const { t } = useI18n();
  const ips = useLoad(() => get<{ ips: UserIp[] }>(`/users/${user.id}/ips`), [user.id]);
  const limit = user.policy.max_ips;
  return (
    <Modal title={t("terminals.detail_title", { name: user.label })} onClose={onClose} wide>
      <StatsOverview userId={user.id} compact />
      <Card title={t("terminals.ips", { n: ips.data?.ips.length ?? 0 })} bodyClass="p-0 pt-3">
        <p className="px-5 pb-2 text-xs text-slate-500">{limit ? t("terminals.ip_limit_on", { n: limit }) : t("terminals.ip_limit_off")}</p>
        <ErrorBox error={ips.error} />
        <table className="w-full text-left text-sm">
          <thead className="bg-slate-50 text-xs text-slate-500 dark:bg-slate-950/50">
            <tr>
              <th className="py-2 pl-5">IP</th>
              <th>{t("terminals.first_seen")}</th>
              <th>{t("terminals.last_seen")}</th>
              <th className="pr-5 text-right">{t("stats.requests")}</th>
            </tr>
          </thead>
          <tbody>
            {ips.data?.ips.map((ip) => (
              <tr key={ip.ip} className="border-t border-slate-100 dark:border-slate-800">
                <td className="py-2 pl-5 font-mono text-xs">{ip.ip}</td>
                <td className="text-xs text-slate-500">{fmtTime(ip.first_seen)}</td>
                <td className="text-xs text-slate-500">{fmtRelative(ip.last_seen)}</td>
                <td className="pr-5 text-right tabular-nums">{fmtNum(ip.requests)}</td>
              </tr>
            ))}
            {ips.data?.ips.length === 0 && (
              <tr>
                <td colSpan={4} className="py-6 text-center text-slate-400">
                  {t("usage.no_data")}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </Card>
    </Modal>
  );
}

function PolicySummary({ p }: { p: Policy }) {
  const { t } = useI18n();
  const parts = [
    p.allowed_models ? <Badge key="m" tone="blue">{t("terminals.allowlist", { m: p.allowed_models.length ? String(p.allowed_models.length) : "0" })}</Badge> : null,
    p.rpm_limit !== null ? <Badge key="r" tone="yellow">{t("terminals.rpm", { n: p.rpm_limit })}</Badge> : null,
    p.daily_token_limit !== null ? <Badge key="d" tone="purple">{t("terminals.daily", { n: fmtNum(p.daily_token_limit) })}</Badge> : null,
    p.account_group ? <Badge key="g">{t("terminals.group", { g: p.account_group })}</Badge> : null,
    p.max_ips ? <Badge key="ip" tone="red">{t("terminals.max_ips_badge", { n: p.max_ips })}</Badge> : null,
  ].filter(Boolean);
  if (!parts.length) return <span className="text-xs text-slate-400">{t("terminals.no_limits")}</span>;
  return <div className="flex flex-wrap gap-1">{parts}</div>;
}

function CreateUserModal({ onClose, onCreated }: { onClose: () => void; onCreated: (label: string, key: string) => void }) {
  const { t } = useI18n();
  const [label, setLabel] = useState("");
  const act = useAction();
  return (
    <Modal title={t("terminals.new_user")} onClose={onClose}>
      <ErrorBox error={act.error} />
      <Field label={t("terminals.label")} hint={t("terminals.label_hint")}>
        <input className={inputClass} value={label} onChange={(e) => setLabel(e.target.value)} autoFocus />
      </Field>
      <div className="flex justify-end gap-2">
        <Button onClick={onClose}>{t("common.cancel")}</Button>
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
          {t("terminals.create")}
        </Button>
      </div>
    </Modal>
  );
}

function PolicyModal({ user, onClose, onDone }: { user: User; onClose: () => void; onDone: () => void }) {
  const { t } = useI18n();
  const toast = useToast();
  const p = user.policy;
  const settings = useLoad(() => get<Settings>("/settings"));
  const [restrict, setRestrict] = useState(p.allowed_models !== null);
  const [models, setModels] = useState<string[]>(p.allowed_models ?? []);
  const [rpm, setRpm] = useState(p.rpm_limit?.toString() ?? "");
  const [daily, setDaily] = useState(p.daily_token_limit?.toString() ?? "");
  const [group, setGroup] = useState(p.account_group ?? "");
  const [maxIps, setMaxIps] = useState(p.max_ips?.toString() ?? "");
  const act = useAction();
  const num = (s: string) => (s.trim() === "" ? null : Number(s));
  const known = settings.data?.models.map((m) => m.id) ?? [];
  const all = Array.from(new Set([...known, ...models]));
  return (
    <Modal title={t("terminals.policy_title", { name: user.label })} onClose={onClose}>
      <ErrorBox error={act.error} />
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" checked={restrict} onChange={(e) => setRestrict(e.target.checked)} />
        {t("terminals.only_models")}
      </label>
      {restrict && (
        <div className="max-h-40 space-y-1 overflow-y-auto rounded-xl border border-slate-200 p-2 text-sm dark:border-slate-800">
          {all.length === 0 && <span className="text-xs text-slate-500">{t("usage.no_data")}</span>}
          {all.map((m) => (
            <label key={m} className="flex items-center gap-2">
              <input type="checkbox" checked={models.includes(m)} onChange={(e) => setModels(e.target.checked ? [...models, m] : models.filter((x) => x !== m))} />
              <code className="text-xs">{m}</code>
            </label>
          ))}
        </div>
      )}
      <div className="grid grid-cols-2 gap-2">
        <Field label={t("terminals.rpm_label")} hint={t("terminals.blank_unlimited")}>
          <input className={inputClass} type="number" min={1} value={rpm} onChange={(e) => setRpm(e.target.value)} />
        </Field>
        <Field label={t("terminals.daily_label")} hint={t("terminals.blank_unlimited")}>
          <input className={inputClass} type="number" min={1} value={daily} onChange={(e) => setDaily(e.target.value)} />
        </Field>
      </div>
      <div className="grid grid-cols-2 gap-2">
        <Field label={t("terminals.group_label")} hint={t("terminals.group_hint")}>
          <input className={inputClass} value={group} onChange={(e) => setGroup(e.target.value)} />
        </Field>
        <Field label={t("terminals.max_ips")} hint={t("terminals.max_ips_hint")}>
          <input className={inputClass} type="number" min={1} value={maxIps} onChange={(e) => setMaxIps(e.target.value)} />
        </Field>
      </div>
      <div className="flex justify-end gap-2">
        <Button onClick={onClose}>{t("common.cancel")}</Button>
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
                max_ips: num(maxIps),
              }),
            );
            if (r !== undefined) {
              toast("success", t("common.saved"));
              onDone();
              onClose();
            }
          }}
        >
          {t("common.save")}
        </Button>
      </div>
    </Modal>
  );
}

function InvitesTab({ invites, error, reload }: { invites: Invite[]; error: string | null; reload: () => Promise<void> }) {
  const { t } = useI18n();
  const toast = useToast();
  const [label, setLabel] = useState("");
  const [ttl, setTtl] = useState("24");
  const [token, setToken] = useState<string | null>(null);
  const act = useAction();
  const now = Date.now() / 1000;
  const register = token ? `curl -X POST ${location.origin}/register -H "Content-Type: application/json" -d '{"token":"${token}","label":"my-laptop"}'` : "";

  return (
    <div className="space-y-4">
      <Card title={t("terminals.invite_create")} icon={<Ticket className="h-5 w-5 text-violet-500" />}>
        <ErrorBox error={act.error} />
        <div className="flex flex-wrap items-end gap-2">
          <div className="min-w-48 flex-1">
            <Field label={t("terminals.invite_label")}>
              <input className={inputClass} value={label} onChange={(e) => setLabel(e.target.value)} placeholder="bob-desktop" />
            </Field>
          </div>
          <div className="w-32">
            <Field label={t("terminals.invite_ttl")}>
              <input className={inputClass} type="number" min={1} value={ttl} onChange={(e) => setTtl(e.target.value)} />
            </Field>
          </div>
          <Button
            tone="primary"
            icon={<Plus className="h-4 w-4" />}
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
            {t("terminals.create")}
          </Button>
        </div>
        {token && (
          <div className="mt-3 space-y-2">
            <SecretOnce label={t("terminals.invite_token")} value={token} />
            <p className="text-xs text-slate-500">{t("terminals.invite_usage")}</p>
            <pre className="overflow-x-auto rounded-xl bg-slate-900 p-3 text-xs text-slate-100">{register}</pre>
          </div>
        )}
      </Card>
      <Card title={t("terminals.invites")} bodyClass="p-0 pt-3">
        <div className="px-4">
          <ErrorBox error={error} />
        </div>
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead className="bg-slate-50 text-xs uppercase tracking-wide text-slate-500 dark:bg-slate-950/50">
              <tr>
                <th className="py-3 pl-4">{t("terminals.invite_label")}</th>
                <th>{t("terminals.created")}</th>
                <th>{t("terminals.expires")}</th>
                <th>{t("terminals.status")}</th>
                <th className="pr-4" />
              </tr>
            </thead>
            <tbody>
              {invites.map((i) => (
                <tr key={i.token_hash} className="border-t border-slate-100 dark:border-slate-800">
                  <td className="py-3 pl-4 font-medium">{i.label}</td>
                  <td className="text-xs text-slate-500">{fmtTime(i.created_at)}</td>
                  <td className="text-xs text-slate-500">{fmtTime(i.expires_at)}</td>
                  <td>
                    {i.redeemed_at ? (
                      <Badge tone="blue">{t("terminals.used", { id: i.user_id ?? "?" })}</Badge>
                    ) : i.expires_at < now ? (
                      <Badge>{t("terminals.expired")}</Badge>
                    ) : (
                      <Badge tone="green">{t("terminals.usable")}</Badge>
                    )}
                  </td>
                  <td className="pr-4 text-right">
                    {!i.redeemed_at && (
                      <IconButton
                        danger
                        title={t("common.delete")}
                        disabled={act.busy}
                        onClick={async () => {
                          const r = await act.run(() => post(`/invites/${i.token_hash}/delete`));
                          if (r !== undefined) toast("success", t("common.done"));
                          await reload();
                        }}
                      >
                        <Trash2 className="h-4 w-4" />
                      </IconButton>
                    )}
                  </td>
                </tr>
              ))}
              {invites.length === 0 && (
                <tr>
                  <td colSpan={5} className="py-10 text-center text-sm text-slate-400">
                    {t("terminals.invite_empty")}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </Card>
    </div>
  );
}
