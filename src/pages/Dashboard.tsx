import { ArrowRight, Bot, CheckCircle2, Download, Gauge, Plus, RefreshCw, Server, TrendingUp, TriangleAlert, Users } from "lucide-react";
import { useState } from "react";
import { navigate } from "../App";
import { displayName, downloadJson, isHealthy, isLow, remaining, resetAt } from "../accounts";
import { get, post } from "../api";
import { fmtNum } from "../format";
import { useI18n } from "../i18n";
import { useToast } from "../toast";
import type { Account, AccountsResponse, Stats, Status } from "../types";
import { Button, Card, ErrorBox, inputClass, QuotaBar, StatCard, useLoad } from "../ui";
import { AddAccountDialog, StatusBadge } from "./Accounts";

export default function Dashboard() {
  const { t } = useI18n();
  const toast = useToast();
  const status = useLoad(() => get<Status>("/status"));
  const accounts = useLoad(() => get<AccountsResponse>("/accounts"));
  const today = useLoad(() => get<Stats>("/stats?days=1"));
  const [adding, setAdding] = useState(false);
  const [switching, setSwitching] = useState(false);

  const list = accounts.data?.accounts ?? [];
  const now = accounts.data?.now ?? Date.now() / 1000;
  const current = list.find((a) => a.id === accounts.data?.recommended);

  const avg = (w: "5h" | "7d") => {
    const vals = list.filter((a) => a.enabled).map((a) => remaining(a.quota, w, now)).filter((v): v is number => v !== null);
    return vals.length ? vals.reduce((s, v) => s + v, 0) / vals.length : null;
  };
  const avg5 = avg("5h");
  const avg7 = avg("7d");
  const low = list.filter((a) => a.enabled && isLow(a, now)).length;
  const day = today.data?.by_day.reduce((s, d) => ({ req: s.req + d.requests, tok: s.tok + d.input_tokens + d.output_tokens }), { req: 0, tok: 0 });

  // Accounts with the most headroom per window.
  const best = (w: "5h" | "7d") =>
    list
      .filter((a) => isHealthy(a, now) && remaining(a.quota, w, now) !== null)
      .sort((a, b) => (remaining(b.quota, w, now) ?? 0) - (remaining(a.quota, w, now) ?? 0))[0];
  const best5 = best("5h");
  const best7 = best("7d");
  const bestOverall = list
    .filter((a) => isHealthy(a, now))
    .sort((a, b) => Math.min(remaining(b.quota, "5h", now) ?? 1, remaining(b.quota, "7d", now) ?? 1) - Math.min(remaining(a.quota, "5h", now) ?? 1, remaining(a.quota, "7d", now) ?? 1))[0];

  const pin = async (a: Account) => {
    try {
      await post(`/accounts/${a.id}/pin`);
      toast("success", t("dashboard.switched", { name: displayName(a) }));
      await accounts.reload();
    } catch (e) {
      toast("error", t("common.failed", { e: String(e) }));
    }
  };

  const pctText = (v: number | null) => (v === null ? "—" : `${Math.round(v * 100)}%`);

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h1 className="text-2xl font-bold tracking-tight">
          {current ? t("dashboard.hello", { name: displayName(current).split("@")[0] }) : t("dashboard.hello_anon")} 👋
        </h1>
        <div className="flex gap-2">
          <Button icon={<Plus className="h-4 w-4" />} onClick={() => setAdding(true)}>
            {t("dashboard.add")}
          </Button>
          <Button
            tone="primary"
            icon={<RefreshCw className={`h-4 w-4 ${accounts.loading ? "animate-spin" : ""}`} />}
            onClick={() => {
              void accounts.reload();
              void status.reload();
              void today.reload();
            }}
          >
            {t("common.refresh")}
          </Button>
        </div>
      </div>
      <ErrorBox error={status.error ?? accounts.error} />

      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <StatCard
          icon={<Users className="h-5 w-5 text-blue-500" />}
          iconBg="bg-blue-50 dark:bg-blue-950"
          value={list.length}
          label={t("dashboard.total_accounts")}
          hint={day && t("dashboard.today", { req: fmtNum(day.req), tok: fmtNum(day.tok) })}
        />
        <StatCard
          icon={<Gauge className="h-5 w-5 text-emerald-500" />}
          iconBg="bg-emerald-50 dark:bg-emerald-950"
          value={pctText(avg5)}
          label={t("dashboard.avg_5h")}
          hint={avg5 !== null && (avg5 >= 0.2 ? `✓ ${t("dashboard.sufficient")}` : t("dashboard.tight"))}
          hintTone={avg5 !== null && avg5 >= 0.2 ? "green" : "red"}
        />
        <StatCard
          icon={<Bot className="h-5 w-5 text-cyan-500" />}
          iconBg="bg-cyan-50 dark:bg-cyan-950"
          value={pctText(avg7)}
          label={t("dashboard.avg_7d")}
          hint={avg7 !== null && (avg7 >= 0.2 ? `✓ ${t("dashboard.sufficient")}` : t("dashboard.tight"))}
          hintTone={avg7 !== null && avg7 >= 0.2 ? "green" : "red"}
        />
        <StatCard
          icon={<TriangleAlert className="h-5 w-5 text-orange-500" />}
          iconBg="bg-orange-50 dark:bg-orange-950"
          value={low}
          label={t("dashboard.low_quota")}
          hint={t("dashboard.low_quota_desc")}
        />
      </div>

      <div className="grid gap-4 lg:grid-cols-2">
        <Card title={t("dashboard.current")} icon={<CheckCircle2 className="h-5 w-5 text-emerald-500" />}>
          {current ? (
            <div className="space-y-4">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium">{displayName(current)}</span>
                <StatusBadge a={current} now={now} />
                {current.plan && <span className="text-xs text-slate-500">{current.plan}</span>}
              </div>
              <QuotaBar label={t("quota.five_hour")} remaining={remaining(current.quota, "5h", now)} resetAt={resetAt(current.quota, "5h")} now={now} />
              <QuotaBar label={t("quota.seven_day")} remaining={remaining(current.quota, "7d", now)} resetAt={resetAt(current.quota, "7d")} now={now} />
              {!current.quota && <p className="text-xs text-slate-400">{t("quota.no_data_hint")}</p>}
              {switching ? (
                <select
                  className={inputClass}
                  autoFocus
                  defaultValue=""
                  onChange={(e) => {
                    const a = list.find((x) => x.id === Number(e.target.value));
                    setSwitching(false);
                    if (a) void pin(a);
                  }}
                  onBlur={() => setSwitching(false)}
                >
                  <option value="" disabled>
                    {t("dashboard.switch")}
                  </option>
                  {list
                    .filter((a) => a.enabled && a.id !== current.id)
                    .map((a) => (
                      <option key={a.id} value={a.id}>
                        #{a.id} {displayName(a)}
                      </option>
                    ))}
                </select>
              ) : (
                <Button className="w-full" onClick={() => setSwitching(true)} disabled={list.filter((a) => a.enabled).length < 2}>
                  {t("dashboard.switch")}
                </Button>
              )}
            </div>
          ) : (
            <p className="text-sm text-slate-500">{t("dashboard.no_current")}</p>
          )}
        </Card>

        <Card title={t("dashboard.best")} icon={<TrendingUp className="h-5 w-5 text-blue-500" />}>
          <div className="space-y-3">
            {[
              { a: best5, w: "5h" as const, label: t("dashboard.best_5h"), cls: "border-emerald-200 bg-emerald-50/60 dark:border-emerald-900 dark:bg-emerald-950/40", tag: "text-emerald-600", pill: "bg-emerald-500" },
              { a: best7, w: "7d" as const, label: t("dashboard.best_7d"), cls: "border-cyan-200 bg-cyan-50/60 dark:border-cyan-900 dark:bg-cyan-950/40", tag: "text-cyan-600", pill: "bg-cyan-500" },
            ].map(({ a, w, label, cls, tag, pill }) => (
              <div key={w} className={`flex items-center justify-between gap-3 rounded-xl border p-3 ${cls}`}>
                <div className="min-w-0">
                  <div className={`text-xs ${tag}`}>{label}</div>
                  <div className="truncate font-medium">{a ? displayName(a) : "—"}</div>
                </div>
                {a && <span className={`rounded-full px-2.5 py-0.5 text-xs font-semibold text-white ${pill}`}>{pctText(remaining(a.quota, w, now))}</span>}
              </div>
            ))}
            {!best5 && !best7 && <p className="text-xs text-slate-400">{t("quota.no_data_hint")}</p>}
            <Button tone="primary" className="w-full" disabled={!bestOverall || bestOverall.id === current?.id} onClick={() => bestOverall && pin(bestOverall)}>
              {t("dashboard.switch_best")}
            </Button>
          </div>
        </Card>
      </div>

      <div className="grid gap-4 md:grid-cols-3">
        <QuickLink
          tone="blue"
          icon={<ArrowRight className="h-4 w-4" />}
          label={t("dashboard.view_all")}
          onClick={() => navigate("accounts")}
        />
        <QuickLink
          tone="purple"
          icon={<Download className="h-4 w-4" />}
          label={t("dashboard.export")}
          onClick={async () => {
            if (!confirm(t("accounts.export_warning"))) return;
            try {
              const data = await get<unknown[]>("/accounts/export");
              downloadJson(`cc-manager-accounts-${new Date().toISOString().slice(0, 10)}.json`, data);
              toast("success", t("accounts.exported", { n: data.length }));
            } catch (e) {
              toast("error", t("common.failed", { e: String(e) }));
            }
          }}
        />
        <QuickLink
          tone="slate"
          icon={<Server className="h-4 w-4" />}
          label={status.data ? t("dashboard.proxy_desc", { listen: status.data.listen, n: status.data.catalog.models }) : t("dashboard.proxy")}
          onClick={() => navigate("connect")}
        />
      </div>

      {adding && <AddAccountDialog onClose={() => setAdding(false)} onDone={accounts.reload} />}
    </div>
  );
}

function QuickLink({ tone, icon, label, onClick }: { tone: "blue" | "purple" | "slate"; icon: React.ReactNode; label: string; onClick: () => void }) {
  const c = {
    blue: "border-blue-100 bg-blue-50/70 text-blue-700 hover:bg-blue-100/70 dark:border-blue-900 dark:bg-blue-950/50 dark:text-blue-300",
    purple: "border-violet-100 bg-violet-50/70 text-violet-700 hover:bg-violet-100/70 dark:border-violet-900 dark:bg-violet-950/50 dark:text-violet-300",
    slate: "border-slate-200 bg-white text-slate-600 hover:bg-slate-50 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-300",
  }[tone];
  return (
    <button onClick={onClick} className={`flex items-center justify-between gap-2 rounded-2xl border px-5 py-4 text-left text-sm font-medium transition-colors ${c}`}>
      <span className="truncate">{label}</span>
      {icon}
    </button>
  );
}
