import { Activity, BarChart3, CalendarClock, CheckCircle2, Coins, DollarSign, Pencil, Plus, RotateCcw, Trash2 } from "lucide-react";
import { useState } from "react";
import { get, post } from "../api";
import { Heatmap, Legend, StackedBars, topSeries } from "../charts";
import { fmtNum } from "../format";
import { useI18n } from "../i18n";
import { useToast } from "../toast";
import type { AccountsResponse, Overview, Price, User } from "../types";
import { Button, Card, ErrorBox, IconButton, Modal, Segmented, StatCard, inputClass, useAction, useLoad } from "../ui";

type Range = "1" | "7" | "30" | "90";
type Gran = "hour" | "day" | "week";
type Group = "none" | "model" | "account" | "user";
type Metric = "tokens" | "requests" | "cost";

const pad = (n: number) => String(n).padStart(2, "0");
const ymd = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;

/// Every bucket label in the range, in local time (the server buckets with
/// the same offset), so quiet periods show as gaps instead of vanishing.
function bucketsFor(days: number, gran: Gran): string[] {
  const out: string[] = [];
  const now = new Date();
  if (gran === "hour") {
    const d = new Date(now);
    d.setMinutes(0, 0, 0);
    for (let i = days * 24 - 1; i >= 0; i--) {
      const x = new Date(d.getTime() - i * 3_600_000);
      out.push(`${ymd(x)} ${pad(x.getHours())}:00`);
    }
  } else if (gran === "day") {
    for (let i = days - 1; i >= 0; i--) out.push(ymd(new Date(now.getTime() - i * 86_400_000)));
  } else {
    const monday = (x: Date) => {
      const y = new Date(x);
      y.setDate(y.getDate() - ((y.getDay() + 6) % 7));
      return ymd(y);
    };
    const seen = new Set<string>();
    for (let i = days - 1; i >= 0; i--) {
      const m = monday(new Date(now.getTime() - i * 86_400_000));
      if (!seen.has(m)) {
        seen.add(m);
        out.push(m);
      }
    }
  }
  return out;
}

export const fmtUsd = (v: number) => (v >= 100 ? `$${v.toFixed(0)}` : v >= 1 ? `$${v.toFixed(2)}` : v > 0 ? `$${v.toFixed(3)}` : "$0");

/// The stats dashboard. With `userId` it shows one terminal and hides the
/// per-terminal split.
export default function StatsOverview({ userId, compact }: { userId?: number; compact?: boolean }) {
  const { t } = useI18n();
  const [range, setRange] = useState<Range>("7");
  const [gran, setGran] = useState<Gran>("day");
  const [group, setGroup] = useState<Group>("model");
  const [metric, setMetric] = useState<Metric>("tokens");
  const [prices, setPrices] = useState(false);
  const days = Number(range);
  const effGran: Gran = gran === "hour" && days > 7 ? "day" : gran;
  const tz = -new Date().getTimezoneOffset();
  const data = useLoad(() => {
    const qs = new URLSearchParams({ days: range, granularity: effGran, group, tz_offset: String(tz) });
    if (userId !== undefined) qs.set("user_id", String(userId));
    return get<Overview>(`/stats/overview?${qs}`);
  }, [range, effGran, group, userId]);
  const accounts = useLoad(() => get<AccountsResponse>("/accounts"));
  const users = useLoad(() => get<{ users: User[] }>("/users"));

  const o = data.data;
  const accountLabel = (k: string) => {
    const a = accounts.data?.accounts.find((x) => String(x.id) === k);
    return k === "0" ? t("stats.no_account") : a ? `#${k} ${a.email || a.label}` : `#${k}`;
  };
  const userLabel = (k: string) => users.data?.users.find((u) => String(u.id) === k)?.label ?? `#${k}`;
  const keyLabel = (k: string) => (group === "account" ? accountLabel(k) : group === "user" ? userLabel(k) : group === "none" ? t("stats.all") : k || "—");
  const value = (x: Overview["totals"]) => (metric === "tokens" ? x.input_tokens + x.output_tokens : metric === "requests" ? x.requests : x.cost_usd);
  const fmt = (v: number) => (metric === "cost" ? fmtUsd(v) : fmtNum(Math.round(v)));

  const labels = bucketsFor(days, effGran);
  // Biggest first by tokens, whatever metric is shown, so colors stay put.
  const tokenTotal = new Map<string, number>();
  for (const p of o?.series ?? []) tokenTotal.set(p.key, (tokenTotal.get(p.key) ?? 0) + p.input_tokens + p.output_tokens);
  const keys = Array.from(tokenTotal.keys()).sort((a, b) => (tokenTotal.get(b) ?? 0) - (tokenTotal.get(a) ?? 0));
  const series = topSeries(
    keys,
    (k) => labels.map((b) => (o?.series ?? []).filter((p) => p.bucket === b && p.key === k).reduce((s, p) => s + value(p), 0)),
    keyLabel,
    t("stats.other"),
  );
  const shortLabels = labels.map((l) => (effGran === "hour" ? l.slice(11, 16) : l.slice(5)));

  const tot = o?.totals;
  const okRate = tot && tot.requests ? (tot.requests - tot.errors) / tot.requests : null;
  const cacheRate = tot && tot.input_tokens + tot.cache_read_tokens > 0 ? tot.cache_read_tokens / (tot.input_tokens + tot.cache_read_tokens) : null;

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center gap-2">
        <Segmented value={range} onChange={setRange} options={(["1", "7", "30", "90"] as Range[]).map((r) => ({ key: r, label: r === "1" ? t("stats.h24") : t("usage.days_short", { n: r }) }))} />
        <Segmented
          value={effGran}
          onChange={setGran}
          options={[
            { key: "hour", label: t("stats.hourly"), title: days > 7 ? t("stats.hourly_hint") : undefined },
            { key: "day", label: t("stats.daily") },
            { key: "week", label: t("stats.weekly") },
          ]}
        />
        {!compact && (
          <Button className="ml-auto" icon={<DollarSign className="h-4 w-4" />} onClick={() => setPrices(true)}>
            {t("stats.prices")}
          </Button>
        )}
      </div>
      <ErrorBox error={data.error} />

      <div className={`grid gap-4 sm:grid-cols-2 ${compact ? "" : "lg:grid-cols-4"}`}>
        <StatCard icon={<Activity className="h-5 w-5 text-blue-500" />} iconBg="bg-blue-50 dark:bg-blue-950" value={fmtNum(tot?.requests ?? 0)} label={t("usage.total_requests")} hint={t("usage.errors_n", { n: fmtNum(tot?.errors ?? 0) })} hintTone={tot?.errors ? "red" : "slate"} />
        <StatCard
          icon={<Coins className="h-5 w-5 text-amber-500" />}
          iconBg="bg-amber-50 dark:bg-amber-950"
          value={fmtNum((tot?.input_tokens ?? 0) + (tot?.output_tokens ?? 0))}
          label={t("usage.total_tokens")}
          hint={t("usage.in_out", { i: fmtNum(tot?.input_tokens ?? 0), o: fmtNum(tot?.output_tokens ?? 0) })}
        />
        <StatCard icon={<DollarSign className="h-5 w-5 text-emerald-500" />} iconBg="bg-emerald-50 dark:bg-emerald-950" value={fmtUsd(tot?.cost_usd ?? 0)} label={t("stats.cost")} hint={t("stats.cost_hint")} />
        <StatCard
          icon={<CheckCircle2 className="h-5 w-5 text-violet-500" />}
          iconBg="bg-violet-50 dark:bg-violet-950"
          value={okRate === null ? "—" : `${(okRate * 100).toFixed(1)}%`}
          label={t("usage.success_rate")}
          hint={cacheRate === null ? undefined : t("stats.cache_rate", { p: `${Math.round(cacheRate * 100)}%` })}
        />
      </div>

      <Card
        title={t("stats.trend")}
        icon={<BarChart3 className="h-5 w-5 text-blue-500" />}
        actions={
          <>
            <Segmented
              value={metric}
              onChange={setMetric}
              options={[
                { key: "tokens", label: "Token" },
                { key: "requests", label: t("stats.requests") },
                { key: "cost", label: t("stats.cost_short") },
              ]}
            />
            <Segmented
              value={group}
              onChange={setGroup}
              options={[
                { key: "none", label: t("stats.all") },
                { key: "model", label: t("usage.model") },
                { key: "account", label: t("usage.account") },
                ...(userId === undefined ? [{ key: "user" as Group, label: t("usage.user") }] : []),
              ]}
            />
          </>
        }
      >
        <div className="mb-3">
          <Legend series={series} />
        </div>
        <StackedBars labels={shortLabels} series={series} format={fmt} empty={t("usage.no_data")} height={compact ? 160 : 220} />
      </Card>

      {o && (
        <div className={`grid gap-4 ${compact ? "" : "lg:grid-cols-2"}`}>
          <RankCard title={t("usage.by_model")} rows={o.by_model} label={(k) => k} metricLabel={t("stats.share")} />
          {!compact && <RankCard title={t("usage.by_account")} rows={o.by_account} label={accountLabel} metricLabel={t("stats.share")} />}
        </div>
      )}

      {o && (
        <div className={`grid gap-4 ${compact || userId !== undefined ? "" : "lg:grid-cols-2"}`}>
          <Card title={t("stats.heatmap")} icon={<CalendarClock className="h-5 w-5 text-blue-500" />}>
            <Heatmap
              cells={o.heatmap}
              weekdays={t("stats.weekdays").split(",")}
              legend={[t("stats.less"), t("stats.more")]}
              format={(c) => `${t("usage.times", { n: c.requests })} · ${fmtNum(c.tokens)} tokens`}
            />
          </Card>
          {userId === undefined && !compact && <RankCard title={t("usage.by_user")} rows={o.by_user} label={userLabel} metricLabel={t("stats.share")} />}
        </div>
      )}
      {prices && <PricesModal onClose={() => setPrices(false)} onSaved={() => void data.reload()} />}
    </div>
  );
}

function RankCard({ title, rows, label, metricLabel }: { title: string; rows: Overview["by_model"]; label: (k: string) => string; metricLabel: string }) {
  const { t } = useI18n();
  const total = Math.max(1, rows.reduce((s, r) => s + r.input_tokens + r.output_tokens, 0));
  return (
    <Card title={title}>
      <div className="overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead className="text-xs text-slate-500">
            <tr>
              <th className="pb-2 font-medium" />
              <th className="pb-2 text-right font-medium">{t("stats.requests")}</th>
              <th className="pb-2 text-right font-medium">Token</th>
              <th className="pb-2 text-right font-medium">{t("stats.cost_short")}</th>
              <th className="w-28 pb-2 pl-3 font-medium">{metricLabel}</th>
            </tr>
          </thead>
          <tbody>
            {rows.slice(0, 10).map((r) => {
              const tok = r.input_tokens + r.output_tokens;
              const share = tok / total;
              return (
                <tr key={r.key} className="border-t border-slate-100 dark:border-slate-800">
                  <td className="max-w-[14rem] truncate py-2" title={label(r.key)}>
                    {label(r.key)}
                  </td>
                  <td className="text-right tabular-nums text-slate-500">{fmtNum(r.requests)}</td>
                  <td className="text-right tabular-nums">{fmtNum(tok)}</td>
                  <td className="text-right tabular-nums">{fmtUsd(r.cost_usd)}</td>
                  <td className="pl-3">
                    <div className="flex items-center gap-2">
                      <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-slate-100 dark:bg-slate-800">
                        <div className="h-full rounded-full" style={{ width: `${share * 100}%`, background: "var(--series-1)" }} />
                      </div>
                      <span className="w-9 text-right text-xs tabular-nums text-slate-500">{Math.round(share * 100)}%</span>
                    </div>
                  </td>
                </tr>
              );
            })}
            {rows.length === 0 && (
              <tr>
                <td colSpan={5} className="py-6 text-center text-slate-400">
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

function PricesModal({ onClose, onSaved }: { onClose: () => void; onSaved: () => void }) {
  const { t } = useI18n();
  const toast = useToast();
  const data = useLoad(() => get<{ prices: Price[]; defaults: Price[] }>("/settings/prices"));
  const [rows, setRows] = useState<Price[] | null>(null);
  const act = useAction();
  const list = rows ?? data.data?.prices ?? [];
  const set = (i: number, patch: Partial<Price>) => setRows(list.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const num = (v: string) => (v === "" ? 0 : Number(v));
  return (
    <Modal title={t("stats.prices")} onClose={onClose} wide>
      <ErrorBox error={data.error ?? act.error} />
      <p className="text-xs text-slate-500">{t("stats.prices_hint")}</p>
      <div className="overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead className="text-xs text-slate-500">
            <tr>
              <th className="pb-2 font-medium">{t("stats.pattern")}</th>
              <th className="pb-2 font-medium">{t("usage.input")}</th>
              <th className="pb-2 font-medium">{t("usage.output")}</th>
              <th className="pb-2 font-medium">{t("usage.cache_read")}</th>
              <th className="pb-2 font-medium">{t("usage.cache_write")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {list.map((p, i) => (
              <tr key={i}>
                <td className="py-1 pr-2">
                  <input className={`${inputClass} min-w-40 font-mono`} value={p.pattern} onChange={(e) => set(i, { pattern: e.target.value })} />
                </td>
                {(["input", "output", "cache_read", "cache_write"] as const).map((k) => (
                  <td key={k} className="py-1 pr-2">
                    <input className={`${inputClass} w-24 tabular-nums`} type="number" min={0} step="0.01" value={p[k]} onChange={(e) => set(i, { [k]: num(e.target.value) })} />
                  </td>
                ))}
                <td>
                  <IconButton danger title={t("common.delete")} onClick={() => setRows(list.filter((_, j) => j !== i))}>
                    <Trash2 className="h-4 w-4" />
                  </IconButton>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="flex flex-wrap justify-between gap-2">
        <div className="flex gap-2">
          <Button small icon={<Plus className="h-3.5 w-3.5" />} onClick={() => setRows([...list, { pattern: "", input: 0, output: 0, cache_read: 0, cache_write: 0 }])}>
            {t("stats.add_price")}
          </Button>
          <Button small icon={<RotateCcw className="h-3.5 w-3.5" />} onClick={() => data.data && setRows(data.data.defaults)}>
            {t("stats.reset_prices")}
          </Button>
        </div>
        <div className="flex gap-2">
          <Button onClick={onClose}>{t("common.cancel")}</Button>
          <Button
            tone="primary"
            icon={<Pencil className="h-4 w-4" />}
            disabled={act.busy || rows === null}
            onClick={async () => {
              const r = await act.run(() => post("/settings/prices", { prices: list }));
              if (r !== undefined) {
                toast("success", t("common.saved"));
                onSaved();
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
