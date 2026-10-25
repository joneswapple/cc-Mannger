import { Ban, Globe, ListChecks, Plus, RefreshCw, Save, ScrollText, Search, ShieldAlert, ShieldCheck, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { get, post } from "../api";
import { fmtNum, fmtRelative, fmtTime } from "../format";
import { useI18n } from "../i18n";
import { useToast } from "../toast";
import type { AccessEntry, IpRule, IpStat, RuntimeView } from "../types";
import { Badge, Button, Card, ErrorBox, Field, IconButton, Pagination, Segmented, SettingRow, StatCard, Switch, inputBase, inputClass, useAction, useLoad } from "../ui";

type Tab = "rules" | "log" | "ips";

export default function Security() {
  const { t } = useI18n();
  const [tab, setTab] = useState<Tab>("rules");
  const rules = useLoad(() => get<{ rules: IpRule[]; allowlist_mode: boolean }>("/security/rules"));
  const ips = useLoad(() => get<{ days: number; ips: IpStat[] }>("/security/ips?days=7"));
  const now = Date.now() / 1000;
  const active = (rules.data?.rules ?? []).filter((r) => !r.expires_at || r.expires_at > now);
  const blocked7 = (ips.data?.ips ?? []).reduce((s, i) => s + i.blocked, 0);

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h1 className="text-2xl font-bold tracking-tight">{t("nav.security")}</h1>
        <Button
          icon={<RefreshCw className={`h-4 w-4 ${rules.loading ? "animate-spin" : ""}`} />}
          onClick={() => {
            void rules.reload();
            void ips.reload();
          }}
        >
          {t("common.refresh")}
        </Button>
      </div>
      <div className="grid gap-4 sm:grid-cols-3">
        <StatCard
          icon={rules.data?.allowlist_mode ? <ShieldCheck className="h-5 w-5 text-emerald-500" /> : <ShieldAlert className="h-5 w-5 text-blue-500" />}
          iconBg={rules.data?.allowlist_mode ? "bg-emerald-50 dark:bg-emerald-950" : "bg-blue-50 dark:bg-blue-950"}
          value={rules.data?.allowlist_mode ? t("security.mode_allowlist") : t("security.mode_open")}
          label={t("security.mode")}
          hint={t("security.rules_n", { n: active.length })}
        />
        <StatCard icon={<Globe className="h-5 w-5 text-violet-500" />} iconBg="bg-violet-50 dark:bg-violet-950" value={fmtNum(ips.data?.ips.length ?? 0)} label={t("security.ips_7d")} />
        <StatCard icon={<Ban className="h-5 w-5 text-red-500" />} iconBg="bg-red-50 dark:bg-red-950" value={fmtNum(blocked7)} label={t("security.blocked_7d")} hintTone="red" />
      </div>
      <Segmented
        value={tab}
        onChange={setTab}
        options={[
          { key: "rules", label: <><ListChecks className="h-4 w-4" />{t("security.tab_rules")}</> },
          { key: "log", label: <><ScrollText className="h-4 w-4" />{t("security.tab_log")}</> },
          { key: "ips", label: <><Globe className="h-4 w-4" />{t("security.tab_ips")}</> },
        ]}
      />
      {tab === "rules" && <RulesTab rules={rules.data?.rules ?? []} error={rules.error} reload={rules.reload} />}
      {tab === "log" && <LogTab />}
      {tab === "ips" && <IpsTab stats={ips.data?.ips ?? []} error={ips.error} reload={async () => { await ips.reload(); await rules.reload(); }} />}
    </div>
  );
}

function RulesTab({ rules, error, reload }: { rules: IpRule[]; error: string | null; reload: () => Promise<void> }) {
  const { t } = useI18n();
  const toast = useToast();
  const [kind, setKind] = useState<"deny" | "allow">("deny");
  const [cidr, setCidr] = useState("");
  const [note, setNote] = useState("");
  const [hours, setHours] = useState(0);
  const act = useAction();
  const now = Date.now() / 1000;
  return (
    <div className="space-y-4">
      <Card title={t("security.add_rule")} icon={<Plus className="h-5 w-5 text-blue-500" />}>
        <ErrorBox error={act.error} />
        <div className="flex flex-wrap items-end gap-2">
          <Segmented
            value={kind}
            onChange={setKind}
            options={[
              { key: "deny", label: t("security.deny") },
              { key: "allow", label: t("security.allow") },
            ]}
          />
          <div className="min-w-48 flex-1">
            <Field label={t("security.cidr")}>
              <input className={`${inputClass} font-mono`} placeholder="203.0.113.7 / 10.0.0.0/8" value={cidr} onChange={(e) => setCidr(e.target.value)} />
            </Field>
          </div>
          <div className="min-w-40 flex-1">
            <Field label={t("security.note")}>
              <input className={inputClass} value={note} onChange={(e) => setNote(e.target.value)} />
            </Field>
          </div>
          <div className="w-36">
            <Field label={t("security.expires")}>
              <select className={inputClass} value={hours} onChange={(e) => setHours(Number(e.target.value))}>
                {[0, 1, 24, 168, 720].map((h) => (
                  <option key={h} value={h}>
                    {h === 0 ? t("security.permanent") : h < 24 ? t("security.hours", { n: h }) : t("terminals.ttl_days", { n: h / 24 })}
                  </option>
                ))}
              </select>
            </Field>
          </div>
          <Button
            tone="primary"
            icon={<Plus className="h-4 w-4" />}
            disabled={act.busy || !cidr.trim()}
            onClick={async () => {
              const r = await act.run(() => post("/security/rules", { kind, cidr, note, expires_hours: hours }));
              if (r !== undefined) {
                toast("success", t("common.done"));
                setCidr("");
                setNote("");
                await reload();
              }
            }}
          >
            {t("security.add")}
          </Button>
        </div>
        <p className="mt-3 text-xs text-slate-500">{t("security.rules_hint")}</p>
      </Card>
      <Card bodyClass="p-0">
        <ErrorBox error={error} />
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead className="bg-slate-50 text-xs uppercase tracking-wide text-slate-500 dark:bg-slate-950/50">
              <tr>
                <th className="py-3 pl-4">{t("security.kind")}</th>
                <th>{t("security.cidr")}</th>
                <th>{t("security.note")}</th>
                <th>{t("security.expires")}</th>
                <th className="text-right">{t("security.hits")}</th>
                <th className="pr-4" />
              </tr>
            </thead>
            <tbody>
              {rules.map((r) => {
                const expired = !!r.expires_at && r.expires_at <= now;
                return (
                  <tr key={r.id} className={`border-t border-slate-100 dark:border-slate-800 ${expired ? "opacity-50" : ""}`}>
                    <td className="py-2.5 pl-4">{r.kind === "deny" ? <Badge tone="red" icon={<Ban className="h-3 w-3" />}>{t("security.deny")}</Badge> : <Badge tone="green" icon={<ShieldCheck className="h-3 w-3" />}>{t("security.allow")}</Badge>}</td>
                    <td className="font-mono text-xs">{r.cidr}</td>
                    <td className="text-slate-500">{r.note || "—"}</td>
                    <td className="text-xs text-slate-500">{r.expires_at ? (expired ? t("security.expired") : fmtRelative(r.expires_at)) : t("security.permanent")}</td>
                    <td className="text-right tabular-nums">{fmtNum(r.hits)}</td>
                    <td className="pr-4 text-right">
                      <IconButton
                        danger
                        title={t("common.delete")}
                        onClick={async () => {
                          const res = await act.run(() => post(`/security/rules/${r.id}/delete`));
                          if (res !== undefined) await reload();
                        }}
                      >
                        <Trash2 className="h-4 w-4" />
                      </IconButton>
                    </td>
                  </tr>
                );
              })}
              {rules.length === 0 && (
                <tr>
                  <td colSpan={6} className="py-10 text-center text-sm text-slate-400">
                    {t("security.no_rules")}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </Card>
      <SecuritySettings />
    </div>
  );
}

function SecuritySettings() {
  const { t } = useI18n();
  const toast = useToast();
  const rt = useLoad(() => get<RuntimeView>("/runtime"));
  const [msg, setMsg] = useState("");
  const [days, setDays] = useState("7");
  const act = useAction();
  const sec = rt.data?.effective.security;
  useEffect(() => {
    if (sec) {
      setMsg(sec.block_message);
      setDays(String(sec.access_log_days));
    }
  }, [sec]);
  const save = async (patch: Record<string, unknown>) => {
    const r = await act.run(() => post("/runtime", { security: patch }));
    if (r !== undefined) toast("success", t("common.saved"));
    await rt.reload();
  };
  if (!sec) return null;
  return (
    <Card title={t("security.settings")} icon={<ShieldAlert className="h-5 w-5 text-slate-500" />}>
      <ErrorBox error={act.error ?? rt.error} />
      <div className="divide-y divide-slate-100 dark:divide-slate-800">
        <SettingRow title={t("security.block_message")} hint={t("security.block_message_hint")}>
          <input className={`${inputBase} w-52 sm:w-72`} value={msg} onChange={(e) => setMsg(e.target.value)} />
          <Button icon={<Save className="h-4 w-4" />} disabled={msg === sec.block_message || !msg.trim()} onClick={() => save({ block_message: msg })}>
            {t("common.save")}
          </Button>
        </SettingRow>
        <SettingRow title={t("security.trust_xff")} hint={t("security.trust_xff_hint")}>
          <Switch checked={sec.trust_forwarded_for} onChange={(v) => save({ trust_forwarded_for: v })} />
        </SettingRow>
        <SettingRow title={t("security.retention")} hint={t("security.retention_hint")}>
          <input className={`${inputBase} w-20`} inputMode="numeric" value={days} onChange={(e) => setDays(e.target.value.replace(/\D/g, ""))} />
          <Button icon={<Save className="h-4 w-4" />} disabled={days === String(sec.access_log_days) || !days} onClick={() => save({ access_log_days: Number(days) })}>
            {t("common.save")}
          </Button>
        </SettingRow>
      </div>
    </Card>
  );
}

const PAGE = 50;

function LogTab() {
  const { t } = useI18n();
  const [ip, setIp] = useState("");
  const [filter, setFilter] = useState<"" | "blocked" | "allowed">("");
  const [page, setPage] = useState(1);
  const log = useLoad(() => get<{ entries: AccessEntry[]; total: number }>(`/security/log?${new URLSearchParams({ ip, filter, page: String(page), page_size: String(PAGE) })}`), [ip, filter, page]);
  return (
    <Card bodyClass="p-0">
      <div className="flex flex-wrap items-center gap-2 border-b border-slate-100 p-4 dark:border-slate-800">
        <Segmented
          value={filter}
          onChange={(v) => {
            setFilter(v);
            setPage(1);
          }}
          options={[
            { key: "", label: t("monitor.all") },
            { key: "allowed", label: t("security.allowed") },
            { key: "blocked", label: t("security.blocked") },
          ]}
        />
        <div className="relative min-w-48 flex-1 sm:max-w-xs">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-400" />
          <input
            className={`${inputBase} w-full pl-9 font-mono`}
            placeholder="IP"
            value={ip}
            onChange={(e) => {
              setIp(e.target.value);
              setPage(1);
            }}
          />
        </div>
        <span className="ml-auto text-xs text-slate-500">{t("security.log_total", { n: log.data?.total ?? 0 })}</span>
      </div>
      <ErrorBox error={log.error} />
      <div className="overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead className="bg-slate-50 text-xs uppercase tracking-wide text-slate-500 dark:bg-slate-950/50">
            <tr>
              <th className="py-3 pl-4">{t("usage.time")}</th>
              <th>IP</th>
              <th>{t("security.request")}</th>
              <th>{t("usage.status")}</th>
              <th className="pr-4">{t("security.result")}</th>
            </tr>
          </thead>
          <tbody>
            {log.data?.entries.map((e) => (
              <tr key={e.id} className="border-t border-slate-100 dark:border-slate-800">
                <td className="whitespace-nowrap py-2 pl-4 text-xs text-slate-500">{fmtTime(e.ts)}</td>
                <td className="font-mono text-xs">{e.ip}</td>
                <td className="font-mono text-xs">
                  {e.method} {e.path}
                </td>
                <td>
                  <Badge tone={e.status < 400 ? "green" : e.status === 429 ? "yellow" : "red"}>{e.status}</Badge>
                </td>
                <td className="pr-4">{e.blocked ? <Badge tone="red" icon={<Ban className="h-3 w-3" />}>{e.rule_id ? t("security.by_rule", { id: e.rule_id }) : t("security.not_allowlisted")}</Badge> : <span className="text-xs text-slate-400">{t("security.allowed")}</span>}</td>
              </tr>
            ))}
            {log.data?.entries.length === 0 && (
              <tr>
                <td colSpan={5} className="py-10 text-center text-sm text-slate-400">
                  {t("usage.no_data")}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      <div className="p-4">
        <Pagination page={page} pageSize={PAGE} total={log.data?.total ?? 0} onChange={setPage} />
      </div>
    </Card>
  );
}

function IpsTab({ stats, error, reload }: { stats: IpStat[]; error: string | null; reload: () => Promise<void> }) {
  const { t } = useI18n();
  const toast = useToast();
  const act = useAction();
  return (
    <Card bodyClass="p-0" title={t("security.ips_title")}>
      <ErrorBox error={error ?? act.error} />
      <div className="overflow-x-auto pt-3">
        <table className="w-full text-left text-sm">
          <thead className="bg-slate-50 text-xs uppercase tracking-wide text-slate-500 dark:bg-slate-950/50">
            <tr>
              <th className="py-3 pl-4">IP</th>
              <th>{t("security.terminals")}</th>
              <th className="text-right">{t("stats.requests")}</th>
              <th className="text-right">{t("usage.error")}</th>
              <th className="text-right">{t("security.blocked")}</th>
              <th>{t("terminals.last_seen")}</th>
              <th className="pr-4" />
            </tr>
          </thead>
          <tbody>
            {stats.map((s) => (
              <tr key={s.ip} className="border-t border-slate-100 dark:border-slate-800">
                <td className="py-2.5 pl-4 font-mono text-xs">{s.ip}</td>
                <td className="max-w-xs">
                  <div className="flex flex-wrap gap-1">
                    {s.terminals.length ? s.terminals.map((x) => <Badge key={x}>{x}</Badge>) : <span className="text-xs text-slate-400">—</span>}
                  </div>
                </td>
                <td className="text-right tabular-nums">{fmtNum(s.requests)}</td>
                <td className="text-right tabular-nums text-slate-500">{fmtNum(s.errors)}</td>
                <td className="text-right tabular-nums">{s.blocked ? <span className="text-red-500">{fmtNum(s.blocked)}</span> : "0"}</td>
                <td className="text-xs text-slate-500">{fmtRelative(s.last_seen)}</td>
                <td className="pr-4 text-right">
                  <IconButton
                    danger
                    title={t("security.block_ip")}
                    onClick={async () => {
                      if (!confirm(t("security.block_confirm", { ip: s.ip }))) return;
                      const r = await act.run(() => post("/security/rules", { kind: "deny", cidr: s.ip, note: t("security.blocked_from_stats") }));
                      if (r !== undefined) toast("success", t("common.done"));
                      await reload();
                    }}
                  >
                    <Ban className="h-4 w-4" />
                  </IconButton>
                </td>
              </tr>
            ))}
            {stats.length === 0 && (
              <tr>
                <td colSpan={7} className="py-10 text-center text-sm text-slate-400">
                  {t("usage.no_data")}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </Card>
  );
}
