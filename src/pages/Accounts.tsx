import {
  ArrowLeftRight,
  Ban,
  Download,
  Grid2x2,
  Info,
  List,
  Lock,
  Plus,
  Power,
  RefreshCw,
  Search,
  ShieldAlert,
  Snowflake,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { displayName, downloadJson, isHealthy, isLow, isProblem, remaining, resetAt } from "../accounts";
import { get, openExternal, post } from "../api";
import { fmtClock, fmtCountdown, fmtDate, fmtTime } from "../format";
import { useI18n } from "../i18n";
import { useToast } from "../toast";
import type { Account, AccountsResponse } from "../types";
import { Badge, Button, Card, ErrorBox, Field, IconButton, inputBase, inputClass, Modal, Pagination, QuotaBar, Segmented, useAction, useLoad } from "../ui";

type Filter = "all" | "available" | "low" | "problem";
const PAGE_SIZE = 10;

export function StatusBadge({ a, now }: { a: Account; now: number }) {
  const { t } = useI18n();
  if (!a.enabled) return <Badge icon={<Ban className="h-3 w-3" />}>{t("status.disabled")}</Badge>;
  if (a.status === "needs_reauth")
    return (
      <Badge tone="red" icon={<Lock className="h-3 w-3" />}>
        {t("status.reauth")}
      </Badge>
    );
  if (a.cooldown_until > now)
    return (
      <Badge tone="yellow" icon={<Snowflake className="h-3 w-3" />}>
        {t("status.cooling", { t: fmtCountdown(a.cooldown_until - now) })}
      </Badge>
    );
  return null;
}

export default function Accounts() {
  const { t } = useI18n();
  const toast = useToast();
  const { data, error, reload, loading } = useLoad(() => get<AccountsResponse>("/accounts"));
  const [view, setView] = useState<"list" | "grid">(() => {
    try {
      const saved = localStorage.getItem("ccm_accounts_view");
      if (saved === "grid" || saved === "list") return saved;
    } catch {
      // fall through to the width-based default
    }
    return window.innerWidth < 768 ? "grid" : "list";
  });
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(1);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [adding, setAdding] = useState(false);
  const [detail, setDetail] = useState<Account | null>(null);
  const act = useAction();

  useEffect(() => {
    try {
      localStorage.setItem("ccm_accounts_view", view);
    } catch {
      // per-viewer preference only
    }
  }, [view]);

  const now = data?.now ?? Date.now() / 1000;
  const all = data?.accounts ?? [];
  const counts = {
    all: all.length,
    available: all.filter((a) => isHealthy(a, now)).length,
    low: all.filter((a) => isLow(a, now)).length,
    problem: all.filter((a) => isProblem(a, now)).length,
  };
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return all.filter((a) => {
      if (q && !`${a.email} ${a.label} ${a.account_group}`.toLowerCase().includes(q)) return false;
      if (filter === "available") return isHealthy(a, now);
      if (filter === "low") return isLow(a, now);
      if (filter === "problem") return isProblem(a, now);
      return true;
    });
  }, [all, query, filter, now]);
  const pageItems = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  useEffect(() => setPage(1), [query, filter]);

  /// Runs an action over accounts, reporting one toast.
  const runMany = async (ids: number[], fn: (id: number) => Promise<unknown>, okMsg: (n: number) => string) => {
    let ok = 0;
    const errors: string[] = [];
    await act.run(async () => {
      for (const id of ids) {
        try {
          await fn(id);
          ok++;
        } catch (e) {
          errors.push(`#${id}: ${e instanceof Error ? e.message : e}`);
        }
      }
    });
    if (ok) toast("success", okMsg(ok));
    if (errors.length) toast("error", errors.join("\n"));
    await reload();
  };

  const exportIds = async (ids: number[]) => {
    if (!confirm(t("accounts.export_warning"))) return;
    try {
      const out = await get<unknown[]>(`/accounts/export${ids.length ? `?ids=${ids.join(",")}` : ""}`);
      downloadJson(`cc-manager-accounts-${new Date().toISOString().slice(0, 10)}.json`, out);
      toast("success", t("accounts.exported", { n: out.length }));
    } catch (e) {
      toast("error", t("common.failed", { e: String(e) }));
    }
  };

  const remove = async (ids: number[]) => {
    const one = all.find((a) => a.id === ids[0]);
    const msg = ids.length === 1 && one ? t("accounts.delete_confirm", { name: displayName(one) }) : t("accounts.delete_many_confirm", { n: ids.length });
    if (!confirm(msg)) return;
    await runMany(ids, (id) => post(`/accounts/${id}/delete`), () => t("accounts.deleted"));
    setSelected(new Set());
  };

  const actions = {
    details: (a: Account) => setDetail(a),
    pin: (a: Account) => runMany([a.id], (id) => post(`/accounts/${id}/pin`), () => t("accounts.pinned")),
    refresh: (a: Account) => runMany([a.id], (id) => post(`/accounts/${id}/refresh`), (n) => t("accounts.refreshed", { n })),
    toggle: (a: Account) => runMany([a.id], (id) => post(`/accounts/${id}`, { enabled: !a.enabled }), () => t("common.done")),
    cooldown: (a: Account) => runMany([a.id], (id) => post(`/accounts/${id}`, { clear_cooldown: true }), () => t("common.done")),
    export: (a: Account) => exportIds([a.id]),
    remove: (a: Account) => remove([a.id]),
  };

  const sel = [...selected].filter((id) => all.some((a) => a.id === id));
  const allOnPageSelected = pageItems.length > 0 && pageItems.every((a) => selected.has(a.id));
  const toggleSel = (id: number) => {
    const s = new Set(selected);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    setSelected(s);
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-3">
        <div className="relative min-w-48 flex-1 sm:max-w-xs">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-400" />
          <input className={`${inputClass} pl-9`} placeholder={t("accounts.search")} value={query} onChange={(e) => setQuery(e.target.value)} />
        </div>
        <Segmented
          value={view}
          onChange={setView}
          options={[
            { key: "list", label: <List className="h-4 w-4" />, title: t("accounts.list") },
            { key: "grid", label: <Grid2x2 className="h-4 w-4" />, title: t("accounts.grid") },
          ]}
        />
        <Segmented
          value={filter}
          onChange={setFilter}
          options={(["all", "available", "low", "problem"] as Filter[]).map((k) => ({ key: k, label: `${t(`accounts.${k}`)} (${counts[k]})` }))}
        />
        <div className="ml-auto flex flex-wrap gap-2">
          <Button icon={<Plus className="h-4 w-4" />} onClick={() => setAdding(true)}>
            {t("accounts.add")}
          </Button>
          <Button
            tone="primary"
            disabled={act.busy || all.length === 0}
            icon={<RefreshCw className={`h-4 w-4 ${act.busy ? "animate-spin" : ""}`} />}
            onClick={() =>
              runMany(sel.length ? sel : all.filter((a) => a.enabled).map((a) => a.id), (id) => post(`/accounts/${id}/refresh`), (n) => t("accounts.refreshed", { n }))
            }
          >
            {sel.length ? t("accounts.refresh_selected") : t("accounts.refresh_all")}
          </Button>
          <Button icon={<Download className="h-4 w-4" />} onClick={() => exportIds(sel)} disabled={all.length === 0}>
            {t("accounts.export")}
          </Button>
        </div>
      </div>

      {sel.length > 0 && (
        <div className="flex flex-wrap items-center gap-2 rounded-xl bg-blue-50 px-4 py-2 text-sm text-blue-800 dark:bg-blue-950 dark:text-blue-200">
          <span className="font-medium">{t("common.selected", { n: sel.length })}</span>
          <Button small onClick={() => runMany(sel, (id) => post(`/accounts/${id}`, { enabled: true }), () => t("common.done"))}>
            {t("common.enable")}
          </Button>
          <Button small onClick={() => runMany(sel, (id) => post(`/accounts/${id}`, { enabled: false }), () => t("common.done"))}>
            {t("common.disable")}
          </Button>
          <Button small tone="danger" onClick={() => remove(sel)}>
            {t("common.delete")}
          </Button>
          <Button small tone="ghost" onClick={() => setSelected(new Set())}>
            {t("common.cancel")}
          </Button>
        </div>
      )}

      <ErrorBox error={error ?? act.error} />

      {view === "list" ? (
        <Card bodyClass="p-0">
          <div className="overflow-x-auto">
            <table className="w-full min-w-[860px] text-left text-sm">
              <thead className="border-b border-slate-100 text-xs font-medium text-slate-500 dark:border-slate-800">
                <tr>
                  <th className="w-10 py-3 pl-5">
                    <input
                      type="checkbox"
                      checked={allOnPageSelected}
                      onChange={() => {
                        const s = new Set(selected);
                        pageItems.forEach((a) => (allOnPageSelected ? s.delete(a.id) : s.add(a.id)));
                        setSelected(s);
                      }}
                    />
                  </th>
                  <th className="py-3">{t("accounts.email")}</th>
                  <th className="w-80 py-3">{t("accounts.quota")}</th>
                  <th className="py-3">{t("accounts.priority")}</th>
                  <th className="py-3">{t("accounts.last_used")}</th>
                  <th className="py-3 pr-5 text-right">{t("common.actions")}</th>
                </tr>
              </thead>
              <tbody>
                {pageItems.map((a) => (
                  <tr key={a.id} className="border-b border-slate-100 last:border-0 hover:bg-slate-50/60 dark:border-slate-800 dark:hover:bg-slate-800/40">
                    <td className="py-3 pl-5 align-middle">
                      <input type="checkbox" checked={selected.has(a.id)} onChange={() => toggleSel(a.id)} />
                    </td>
                    <td className="py-3 align-middle">
                      <AccountName a={a} now={now} current={a.id === data?.recommended} />
                    </td>
                    <td className="py-3 pr-6 align-middle">
                      <AccountQuota a={a} now={now} />
                    </td>
                    <td className="py-3 align-middle text-slate-600 dark:text-slate-300">
                      {a.priority} / {a.weight}
                    </td>
                    <td className="py-3 align-middle font-mono text-xs text-slate-500">
                      {a.last_used_at ? (
                        <>
                          <div>{fmtDate(a.last_used_at)}</div>
                          <div className="text-slate-400">{fmtClock(a.last_used_at)}</div>
                        </>
                      ) : (
                        t("common.never")
                      )}
                    </td>
                    <td className="py-3 pr-5 align-middle">
                      <RowActions a={a} now={now} isCurrent={a.id === data?.recommended} busy={act.busy} on={actions} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {filtered.length === 0 && !loading && <Empty all={all.length === 0} />}
          <div className="border-t border-slate-100 px-5 py-3 dark:border-slate-800">
            <Pagination page={page} pageSize={PAGE_SIZE} total={filtered.length} onChange={setPage} />
          </div>
        </Card>
      ) : (
        <>
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {pageItems.map((a) => (
              <div
                key={a.id}
                className={`rounded-2xl border bg-white p-4 shadow-sm dark:bg-slate-900 ${
                  selected.has(a.id) ? "border-blue-400 ring-2 ring-blue-100 dark:ring-blue-950" : "border-slate-200/80 dark:border-slate-800"
                }`}
              >
                <div className="flex items-start gap-3">
                  <input type="checkbox" className="mt-1" checked={selected.has(a.id)} onChange={() => toggleSel(a.id)} />
                  <div className="min-w-0 flex-1">
                    <AccountName a={a} now={now} current={a.id === data?.recommended} />
                  </div>
                </div>
                <div className="mt-4">
                  <AccountQuota a={a} now={now} />
                </div>
                <div className="mt-3 flex items-center justify-between border-t border-slate-100 pt-2 dark:border-slate-800">
                  <span className="font-mono text-xs text-slate-400">
                    {a.last_used_at ? `${fmtDate(a.last_used_at)} ${fmtClock(a.last_used_at)}` : t("common.never")}
                  </span>
                  <RowActions a={a} now={now} isCurrent={a.id === data?.recommended} busy={act.busy} on={actions} />
                </div>
              </div>
            ))}
          </div>
          {filtered.length === 0 && !loading && (
            <Card>
              <Empty all={all.length === 0} />
            </Card>
          )}
          <Pagination page={page} pageSize={PAGE_SIZE} total={filtered.length} onChange={setPage} />
        </>
      )}

      {data && (
        <p className="text-xs text-slate-400">
          {t("accounts.strategy", { s: data.pool.strategy, sticky: data.pool.sticky ? "✓" : "✗", n: data.pool.max_retries })}
        </p>
      )}

      {adding && <AddAccountDialog onClose={() => setAdding(false)} onDone={reload} />}
      {detail && <DetailDialog account={detail} now={now} onClose={() => setDetail(null)} onDone={reload} />}
    </div>
  );
}

function Empty({ all }: { all: boolean }) {
  const { t } = useI18n();
  return <p className="px-5 py-10 text-center text-sm text-slate-500">{all ? t("accounts.empty_all") : t("accounts.empty")}</p>;
}

function AccountName({ a, now, current }: { a: Account; now: number; current: boolean }) {
  const { t } = useI18n();
  return (
    <div className="min-w-0">
      <div className="flex flex-wrap items-center gap-1.5">
        <span className={`truncate font-medium ${current ? "text-blue-600 dark:text-blue-400" : ""}`}>{displayName(a)}</span>
        {current && <Badge tone="blue">{t("status.current")}</Badge>}
        <StatusBadge a={a} now={now} />
      </div>
      <div className="mt-0.5 flex flex-wrap items-center gap-1.5 text-xs text-slate-500">
        <span>#{a.id}</span>
        {a.email && a.label !== a.email && <span className="truncate">{a.label}</span>}
        {a.plan && <Badge tone="purple">{a.plan.replace(/^claude_/, "")}</Badge>}
        {a.account_group && <Badge>{t("accounts.group", { g: a.account_group })}</Badge>}
        {a.proxy_url && <Badge>{t("accounts.proxy")}</Badge>}
        {a.sync_file && <Badge>{t("accounts.synced")}</Badge>}
      </div>
    </div>
  );
}

function AccountQuota({ a, now }: { a: Account; now: number }) {
  const { t } = useI18n();
  if (a.status === "needs_reauth")
    return (
      <div className="flex items-center gap-2 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-600 dark:border-red-900 dark:bg-red-950 dark:text-red-300">
        <ShieldAlert className="h-4 w-4 shrink-0" />
        <span className="line-clamp-2">{a.status_reason || t("status.reauth")}</span>
      </div>
    );
  return (
    <div className="space-y-2">
      <QuotaBar compact label={t("quota.five_hour")} remaining={remaining(a.quota, "5h", now)} resetAt={resetAt(a.quota, "5h")} now={now} />
      <QuotaBar compact label={t("quota.seven_day")} remaining={remaining(a.quota, "7d", now)} resetAt={resetAt(a.quota, "7d")} now={now} />
    </div>
  );
}

type Actions = Record<"details" | "pin" | "refresh" | "toggle" | "cooldown" | "export" | "remove", (a: Account) => void>;

function RowActions({ a, now, isCurrent, busy, on }: { a: Account; now: number; isCurrent: boolean; busy: boolean; on: Actions }) {
  const { t } = useI18n();
  return (
    <div className="flex items-center justify-end gap-0.5">
      <IconButton title={t("common.details")} onClick={() => on.details(a)}>
        <Info className="h-4 w-4" />
      </IconButton>
      {!isCurrent && (
        <IconButton title={t("accounts.pin")} disabled={busy || !a.enabled} onClick={() => on.pin(a)}>
          <ArrowLeftRight className="h-4 w-4" />
        </IconButton>
      )}
      <IconButton title={t("common.refresh")} disabled={busy} onClick={() => on.refresh(a)}>
        <RefreshCw className="h-4 w-4" />
      </IconButton>
      {a.cooldown_until > now && (
        <IconButton title={t("accounts.clear_cooldown")} disabled={busy} onClick={() => on.cooldown(a)}>
          <Snowflake className="h-4 w-4" />
        </IconButton>
      )}
      <IconButton title={a.enabled ? t("common.disable") : t("common.enable")} disabled={busy} onClick={() => on.toggle(a)}>
        <Power className={`h-4 w-4 ${a.enabled ? "" : "text-red-400"}`} />
      </IconButton>
      <IconButton title={t("common.export")} onClick={() => on.export(a)}>
        <Download className="h-4 w-4" />
      </IconButton>
      <IconButton title={t("common.delete")} danger disabled={busy} onClick={() => on.remove(a)}>
        <Trash2 className="h-4 w-4" />
      </IconButton>
    </div>
  );
}

export function AddAccountDialog({ onClose, onDone }: { onClose: () => void; onDone: () => void }) {
  const { t } = useI18n();
  const toast = useToast();
  const [tab, setTab] = useState<"oauth" | "json" | "token">("oauth");
  const [label, setLabel] = useState("");
  const [group, setGroup] = useState("");
  const [start, setStart] = useState<{ url: string; state: string } | null>(null);
  const [code, setCode] = useState("");
  const [text, setText] = useState("");
  const [access, setAccess] = useState("");
  const [refresh, setRefresh] = useState("");
  const [result, setResult] = useState<{ imported: Account[]; errors: { index: number; error: string }[] } | null>(null);
  const act = useAction();

  const finish = (a: Account) => {
    toast("success", t("add.added", { name: displayName(a) }));
    onDone();
    onClose();
  };

  const doImport = async (credentials: unknown) => {
    const r = await act.run(() =>
      post<{ imported: Account[]; errors: { index: number; error: string }[] }>("/accounts/import", { credentials, label, account_group: group }),
    );
    if (!r) return;
    setResult(r);
    onDone();
    if (r.imported.length) toast("success", t("add.imported", { n: r.imported.length }));
    if (r.imported.length && !r.errors.length) onClose();
  };

  return (
    <Modal title={t("add.title")} onClose={onClose}>
      <Segmented
        value={tab}
        onChange={(k) => {
          setTab(k);
          act.setError(null);
        }}
        options={[
          { key: "oauth", label: t("add.tab_oauth") },
          { key: "json", label: t("add.tab_json") },
          { key: "token", label: t("add.tab_token") },
        ]}
      />
      <ErrorBox error={act.error} />
      <div className="grid grid-cols-2 gap-3">
        <Field label={t("add.label")} hint={t("add.label_hint")}>
          <input className={inputClass} value={label} onChange={(e) => setLabel(e.target.value)} />
        </Field>
        <Field label={t("add.group")}>
          <input className={inputClass} value={group} onChange={(e) => setGroup(e.target.value)} />
        </Field>
      </div>

      {tab === "oauth" &&
        (!start ? (
          <Button
            tone="primary"
            className="w-full"
            disabled={act.busy}
            onClick={async () => {
              const s = await act.run(() => post<{ url: string; state: string }>("/accounts/oauth/start", { label, account_group: group }));
              if (s) {
                setStart(s);
                void openExternal(s.url);
              }
            }}
          >
            {t("add.start")}
          </Button>
        ) : (
          <div className="space-y-3">
            <p className="text-sm">{t("add.step1")}</p>
            <div className="flex gap-2">
              <input className={`${inputClass} font-mono text-xs`} readOnly value={start.url} onFocus={(e) => e.target.select()} />
              <Button small onClick={() => openExternal(start.url)}>
                {t("add.open")}
              </Button>
              <Button small onClick={() => navigator.clipboard.writeText(start.url).catch(() => {})}>
                {t("common.copy")}
              </Button>
            </div>
            <p className="text-sm">{t("add.step2")}</p>
            <input className={`${inputClass} font-mono`} value={code} onChange={(e) => setCode(e.target.value)} placeholder="code#state" />
            <Button
              tone="primary"
              className="w-full"
              disabled={act.busy || !code.trim()}
              onClick={async () => {
                const a = await act.run(() => post<Account>("/accounts/oauth/complete", { code: code.trim(), state: start.state }));
                if (a) finish(a);
              }}
            >
              {t("add.finish")}
            </Button>
          </div>
        ))}

      {tab === "json" && (
        <div className="space-y-3">
          <p className="text-xs text-slate-500">{t("add.json_hint")}</p>
          <textarea className={`${inputClass} h-40 font-mono text-xs`} value={text} onChange={(e) => setText(e.target.value)} placeholder='{"claudeAiOauth": {...}}' />
          <div className="flex items-center justify-between gap-2">
            <label className={`${inputBase} cursor-pointer text-xs`}>
              {t("add.choose_file")}
              <input
                type="file"
                accept=".json,application/json"
                className="hidden"
                onChange={async (e) => {
                  const f = e.target.files?.[0];
                  if (f) setText(await f.text());
                }}
              />
            </label>
            <Button
              tone="primary"
              disabled={act.busy || !text.trim()}
              onClick={() => {
                try {
                  void doImport(JSON.parse(text));
                } catch {
                  act.setError(t("add.invalid_json"));
                }
              }}
            >
              {t("add.import_btn")}
            </Button>
          </div>
        </div>
      )}

      {tab === "token" && (
        <div className="space-y-3">
          <Field label={t("add.access_token")}>
            <input className={`${inputClass} font-mono text-xs`} value={access} onChange={(e) => setAccess(e.target.value)} placeholder="sk-ant-oat…" />
          </Field>
          <Field label={t("add.refresh_token")}>
            <input className={`${inputClass} font-mono text-xs`} value={refresh} onChange={(e) => setRefresh(e.target.value)} placeholder="sk-ant-ort…" />
          </Field>
          <Button
            tone="primary"
            className="w-full"
            disabled={act.busy || !access.trim()}
            onClick={() =>
              doImport({
                access_token: access.trim(),
                refresh_token: refresh.trim(),
                // Unknown expiry: refresh on first use when a refresh token is given.
                expires_in: refresh.trim() ? 1 : 3600,
              })
            }
          >
            {t("add.import_btn")}
          </Button>
        </div>
      )}

      {result && result.errors.length > 0 && (
        <div className="space-y-1 text-xs text-red-600">
          {result.errors.map((e) => (
            <div key={e.index}>{t("add.item_error", { i: e.index + 1, e: e.error })}</div>
          ))}
        </div>
      )}
    </Modal>
  );
}

function DetailDialog({ account, now, onClose, onDone }: { account: Account; now: number; onClose: () => void; onDone: () => void }) {
  const { t } = useI18n();
  const toast = useToast();
  const [f, setF] = useState({
    label: account.label,
    priority: String(account.priority),
    weight: String(account.weight),
    account_group: account.account_group,
    proxy_url: account.proxy_url,
  });
  const act = useAction();
  const set = (k: keyof typeof f) => (e: React.ChangeEvent<HTMLInputElement>) => setF({ ...f, [k]: e.target.value });
  const rows: [string, React.ReactNode][] = [
    [t("accounts.email"), account.email || t("accounts.unknown_email")],
    [t("detail.plan"), account.plan || "—"],
    [t("detail.org"), <code className="text-xs">{account.org_uuid || "—"}</code>],
    [t("detail.status_reason"), account.status_reason || "—"],
    [t("detail.source"), <code className="break-all text-xs">{account.source_path || "—"}</code>],
    [t("detail.quota_status"), account.quota?.status || "—"],
    [t("detail.cooldown_until"), account.cooldown_until > now ? fmtTime(account.cooldown_until) : "—"],
    [t("accounts.last_used"), fmtTime(account.last_used_at)],
    [t("detail.created"), fmtTime(account.created_at)],
    [t("detail.updated"), fmtTime(account.updated_at)],
  ];
  return (
    <Modal title={t("detail.title", { id: account.id })} onClose={onClose} wide>
      <ErrorBox error={act.error} />
      <div className="grid gap-5 md:grid-cols-2">
        <div className="space-y-4">
          <div className="space-y-2 rounded-xl bg-slate-50 p-4 dark:bg-slate-800/50">
            <QuotaBar label={t("quota.five_hour")} remaining={remaining(account.quota, "5h", now)} resetAt={resetAt(account.quota, "5h")} now={now} />
            <QuotaBar label={t("quota.seven_day")} remaining={remaining(account.quota, "7d", now)} resetAt={resetAt(account.quota, "7d")} now={now} />
          </div>
          <dl className="grid grid-cols-[7rem_1fr] gap-x-3 gap-y-2 text-sm">
            {rows.map(([k, v]) => (
              <div key={k} className="contents">
                <dt className="text-slate-500">{k}</dt>
                <dd className="min-w-0 break-words">{v}</dd>
              </div>
            ))}
          </dl>
        </div>
        <div className="space-y-3">
          <Field label={t("detail.label")}>
            <input className={inputClass} value={f.label} onChange={set("label")} />
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field label={t("detail.priority")} hint={t("detail.priority_hint")}>
              <input className={inputClass} type="number" value={f.priority} onChange={set("priority")} />
            </Field>
            <Field label={t("detail.weight")} hint={t("detail.weight_hint")}>
              <input className={inputClass} type="number" min={1} value={f.weight} onChange={set("weight")} />
            </Field>
          </div>
          <Field label={t("detail.group")} hint={t("detail.group_hint")}>
            <input className={inputClass} value={f.account_group} onChange={set("account_group")} />
          </Field>
          <Field label={t("detail.proxy")} hint={t("detail.proxy_hint")}>
            <input className={`${inputClass} font-mono`} value={f.proxy_url} onChange={set("proxy_url")} placeholder="socks5://127.0.0.1:1080" />
          </Field>
          <Button
            tone="primary"
            className="w-full"
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
                toast("success", t("common.saved"));
                onDone();
                onClose();
              }
            }}
          >
            {t("common.save")}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
