import { Activity, BarChart3, CheckCircle2, Coins, Database, RefreshCw, Search, Timer } from "lucide-react";
import { useState } from "react";
import { get } from "../api";
import { fmtNum, fmtTime } from "../format";
import { useI18n } from "../i18n";
import type { Stats, UsageEvent, UsagePage } from "../types";
import { Badge, Button, Card, ErrorBox, Modal, Pagination, Segmented, StatCard, inputBase, useLoad } from "../ui";

const PAGE_SIZE = 20;

export default function Usage() {
  const { t } = useI18n();
  const [days, setDays] = useState(7);
  const stats = useLoad(() => get<Stats>(`/stats?days=${days}`), [days]);
  const [filter, setFilter] = useState({ q: "", model: "", status: "", sort: "time_desc" });
  const [applied, setApplied] = useState(filter);
  const [page, setPage] = useState(1);
  const [detail, setDetail] = useState<UsageEvent | null>(null);
  const usage = useLoad(() => {
    const qs = new URLSearchParams({ ...applied, page: String(page), page_size: String(PAGE_SIZE) });
    return get<UsagePage>(`/usage?${qs}`);
  }, [applied, page]);

  const byDay = stats.data?.by_day ?? [];
  const sum = byDay.reduce(
    (s, d) => ({
      req: s.req + d.requests,
      inp: s.inp + d.input_tokens,
      out: s.out + d.output_tokens,
      cache: s.cache + d.cache_read_tokens,
      err: s.err + d.errors,
      lat: s.lat + d.avg_latency_ms * d.requests,
    }),
    { req: 0, inp: 0, out: 0, cache: 0, err: 0, lat: 0 },
  );
  const okRate = sum.req ? (sum.req - sum.err) / sum.req : null;

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h1 className="text-2xl font-bold tracking-tight">{t("nav.usage")}</h1>
        <div className="flex flex-wrap items-center gap-2">
          <Segmented
            value={String(days)}
            onChange={(k) => setDays(Number(k))}
            options={[1, 7, 30, 90].map((d) => ({ key: String(d), label: t("usage.days_short", { n: d }) }))}
          />
          <Button
            icon={<RefreshCw className={`h-4 w-4 ${stats.loading || usage.loading ? "animate-spin" : ""}`} />}
            onClick={() => {
              void stats.reload();
              void usage.reload();
            }}
          >
            {t("common.refresh")}
          </Button>
        </div>
      </div>
      <ErrorBox error={stats.error} />

      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <StatCard icon={<Activity className="h-5 w-5 text-blue-500" />} iconBg="bg-blue-50 dark:bg-blue-950" value={fmtNum(sum.req)} label={t("usage.total_requests")} hint={t("usage.last_days", { n: days })} />
        <StatCard
          icon={<Coins className="h-5 w-5 text-amber-500" />}
          iconBg="bg-amber-50 dark:bg-amber-950"
          value={fmtNum(sum.inp + sum.out)}
          label={t("usage.total_tokens")}
          hint={t("usage.in_out", { i: fmtNum(sum.inp), o: fmtNum(sum.out) })}
        />
        <StatCard
          icon={<CheckCircle2 className="h-5 w-5 text-emerald-500" />}
          iconBg="bg-emerald-50 dark:bg-emerald-950"
          value={okRate === null ? "—" : `${(okRate * 100).toFixed(1)}%`}
          label={t("usage.success_rate")}
          hint={t("usage.errors_n", { n: fmtNum(sum.err) })}
          hintTone={sum.err ? "red" : "slate"}
        />
        <StatCard
          icon={<Timer className="h-5 w-5 text-violet-500" />}
          iconBg="bg-violet-50 dark:bg-violet-950"
          value={sum.req ? `${(sum.lat / sum.req / 1000).toFixed(1)}s` : "—"}
          label={t("usage.avg_latency")}
          hint={t("usage.cache_hits", { n: fmtNum(sum.cache) })}
        />
      </div>

      <Card title={t("usage.daily_tokens")} icon={<BarChart3 className="h-5 w-5 text-blue-500" />}>
        <DailyChart rows={fillDays(byDay, days)} />
      </Card>

      {stats.data && (
        <div className="grid gap-4 lg:grid-cols-3">
          <Rollup title={t("usage.by_model")} rows={stats.data.by_model.map((r) => ({ name: r.model || "—", ...r }))} />
          <Rollup title={t("usage.by_user")} rows={stats.data.by_user.map((r) => ({ name: r.label, ...r }))} />
          <Rollup title={t("usage.by_account")} rows={stats.data.by_account.map((r) => ({ name: `#${r.account_id} ${r.label}`, ...r }))} />
        </div>
      )}

      <Card title={t("usage.records", { n: usage.data?.total ?? 0 })} icon={<Database className="h-5 w-5 text-slate-500" />} bodyClass="p-0 pt-3">
        <form
          className="flex flex-wrap gap-2 px-5 pb-3"
          onSubmit={(e) => {
            e.preventDefault();
            setPage(1);
            setApplied(filter);
          }}
        >
          <div className="relative min-w-48 flex-1 sm:max-w-xs">
            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-400" />
            <input className={`${inputBase} w-full pl-9`} placeholder={t("usage.search")} value={filter.q} onChange={(e) => setFilter({ ...filter, q: e.target.value })} />
          </div>
          <input className={`${inputBase} w-40`} placeholder={t("usage.model")} value={filter.model} onChange={(e) => setFilter({ ...filter, model: e.target.value })} />
          <select className={`${inputBase} w-32`} value={filter.status} onChange={(e) => setFilter({ ...filter, status: e.target.value })}>
            <option value="">{t("usage.all_status")}</option>
            <option value="ok">{t("usage.ok")}</option>
            <option value="error">{t("usage.error")}</option>
            <option value="429">429</option>
            <option value="5xx">5xx</option>
          </select>
          <select className={`${inputBase} w-36`} value={filter.sort} onChange={(e) => setFilter({ ...filter, sort: e.target.value })}>
            <option value="time_desc">{t("usage.newest")}</option>
            <option value="tokens_desc">{t("usage.most_tokens")}</option>
            <option value="latency_desc">{t("usage.slowest")}</option>
          </select>
          <Button type="submit" tone="primary">
            {t("usage.query")}
          </Button>
        </form>
        <div className="px-5">
          <ErrorBox error={usage.error} />
        </div>
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead className="bg-slate-50 text-xs uppercase tracking-wide text-slate-500 dark:bg-slate-950/50">
              <tr>
                <th className="py-3 pl-5">{t("usage.status")}</th>
                <th>{t("usage.time")}</th>
                <th>{t("usage.model")}</th>
                <th>{t("usage.user")}</th>
                <th>{t("usage.account")}</th>
                <th className="text-right">{t("usage.input")}</th>
                <th className="text-right">{t("usage.output")}</th>
                <th className="text-right">{t("usage.cache_read")}</th>
                <th className="pr-5 text-right">{t("usage.latency")}</th>
              </tr>
            </thead>
            <tbody>
              {usage.data?.events.map((e) => (
                <tr
                  key={e.id}
                  onClick={() => setDetail(e)}
                  className="cursor-pointer border-t border-slate-100 hover:bg-slate-50/60 dark:border-slate-800 dark:hover:bg-slate-800/30"
                >
                  <td className="py-2.5 pl-5">
                    <StatusPill status={e.status} />
                  </td>
                  <td className="whitespace-nowrap text-xs text-slate-500">{fmtTime(e.ts)}</td>
                  <td className="font-mono text-xs">{e.model}</td>
                  <td>{e.user_label}</td>
                  <td className="text-slate-500">{e.account_id ? `#${e.account_id}` : "—"}</td>
                  <td className="text-right tabular-nums">{fmtNum(e.input_tokens)}</td>
                  <td className="text-right tabular-nums">{fmtNum(e.output_tokens)}</td>
                  <td className="text-right tabular-nums text-slate-500">{fmtNum(e.cache_read_tokens)}</td>
                  <td className="pr-5 text-right tabular-nums">{(e.latency_ms / 1000).toFixed(1)}s</td>
                </tr>
              ))}
              {usage.data?.events.length === 0 && (
                <tr>
                  <td colSpan={9} className="py-10 text-center text-sm text-slate-400">
                    {t("usage.no_data")}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
        <div className="p-4">
          <Pagination page={page} pageSize={PAGE_SIZE} total={usage.data?.total ?? 0} onChange={setPage} />
        </div>
      </Card>
      {detail && <DetailModal e={detail} onClose={() => setDetail(null)} />}
    </div>
  );
}

/// One row per UTC day of the window (the server groups by UTC date), with
/// zeros for days without traffic.
function fillDays(rows: Stats["by_day"], days: number): Stats["by_day"] {
  if (rows.length === 0) return rows;
  const byDate = new Map(rows.map((r) => [r.date, r]));
  const out: Stats["by_day"] = [];
  for (let i = days - 1; i >= 0; i--) {
    const date = new Date(Date.now() - i * 86_400_000).toISOString().slice(0, 10);
    out.push(byDate.get(date) ?? { date, requests: 0, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, errors: 0, avg_latency_ms: 0 });
  }
  return out;
}

function StatusPill({ status }: { status: number }) {
  return <Badge tone={status < 400 ? "green" : status === 429 ? "yellow" : "red"}>{status}</Badge>;
}

/// Stacked input / output bars per day, with a hover tooltip.
function DailyChart({ rows }: { rows: Stats["by_day"] }) {
  const { t } = useI18n();
  const [hover, setHover] = useState<number | null>(null);
  if (rows.length === 0) return <p className="py-8 text-center text-sm text-slate-400">{t("usage.no_data")}</p>;
  const max = Math.max(1, ...rows.map((d) => d.input_tokens + d.output_tokens));
  const h = hover === null ? null : rows[hover];
  return (
    <div>
      <div className="mb-3 flex flex-wrap items-center gap-4 text-xs text-slate-500">
        <span className="flex items-center gap-1.5">
          <span className="h-2.5 w-2.5 rounded-sm bg-blue-500" />
          {t("usage.input")}
        </span>
        <span className="flex items-center gap-1.5">
          <span className="h-2.5 w-2.5 rounded-sm bg-violet-400" />
          {t("usage.output")}
        </span>
        <span className="ml-auto font-mono">
          {h ? `${h.date} · ${t("usage.times", { n: h.requests })} · ${fmtNum(h.input_tokens)} / ${fmtNum(h.output_tokens)}` : t("usage.peak", { n: fmtNum(max) })}
        </span>
      </div>
      <div className="flex h-44 items-end gap-1.5 border-b border-slate-200 dark:border-slate-800" onMouseLeave={() => setHover(null)}>
        {rows.map((d, i) => {
          const total = d.input_tokens + d.output_tokens;
          return (
            <div key={d.date} className="group flex h-full max-w-16 flex-1 flex-col justify-end" onMouseEnter={() => setHover(i)}>
              <div className={`flex w-full flex-col overflow-hidden rounded-t-md transition-opacity ${hover !== null && hover !== i ? "opacity-50" : ""}`} style={{ height: `${(total / max) * 100}%`, minHeight: total ? 3 : 0 }}>
                <div className="w-full bg-violet-400" style={{ flex: d.output_tokens }} />
                <div className="w-full bg-blue-500" style={{ flex: d.input_tokens }} />
              </div>
            </div>
          );
        })}
      </div>
      <div className="mt-1 flex gap-1.5">
        {rows.map((d, i) => (
          <div key={d.date} className="max-w-16 flex-1 truncate text-center text-[10px] text-slate-400">
            {rows.length <= 14 || i % Math.ceil(rows.length / 10) === 0 ? d.date.slice(5) : ""}
          </div>
        ))}
      </div>
    </div>
  );
}

function Rollup({ title, rows }: { title: string; rows: { name: string; requests: number; input_tokens: number; output_tokens: number }[] }) {
  const { t } = useI18n();
  const sorted = [...rows].sort((a, b) => b.input_tokens + b.output_tokens - (a.input_tokens + a.output_tokens));
  const total = Math.max(1, sorted.reduce((s, r) => s + r.input_tokens + r.output_tokens, 0));
  return (
    <Card title={title}>
      <div className="space-y-3">
        {sorted.slice(0, 8).map((r) => {
          const tok = r.input_tokens + r.output_tokens;
          return (
            <div key={r.name} className="text-sm">
              <div className="mb-1 flex items-center justify-between gap-2">
                <span className="truncate" title={r.name}>
                  {r.name}
                </span>
                <span className="shrink-0 text-xs text-slate-500">
                  {t("usage.times", { n: r.requests })} · <span className="font-medium text-slate-700 dark:text-slate-200">{fmtNum(tok)}</span>
                </span>
              </div>
              <div className="h-1.5 w-full overflow-hidden rounded-full bg-slate-100 dark:bg-slate-800">
                <div className="h-full rounded-full bg-blue-500" style={{ width: `${(tok / total) * 100}%` }} />
              </div>
            </div>
          );
        })}
        {sorted.length === 0 && <p className="text-sm text-slate-400">{t("usage.no_data")}</p>}
      </div>
    </Card>
  );
}

function DetailModal({ e, onClose }: { e: UsageEvent; onClose: () => void }) {
  const { t } = useI18n();
  const rows: [string, React.ReactNode][] = [
    [t("usage.status"), <StatusPill key="s" status={e.status} />],
    [t("usage.time"), fmtTime(e.ts)],
    [t("usage.model"), <code key="m">{e.model}</code>],
    [t("usage.user"), `${e.user_label} (#${e.user_id}, key #${e.key_id})`],
    [t("usage.account"), e.account_id ? `#${e.account_id}` : "—"],
    [t("usage.input"), e.input_tokens.toLocaleString()],
    [t("usage.output"), e.output_tokens.toLocaleString()],
    [t("usage.cache_read"), e.cache_read_tokens.toLocaleString()],
    [t("usage.cache_write"), e.cache_creation_tokens.toLocaleString()],
    [t("usage.latency"), `${e.latency_ms.toLocaleString()} ms`],
  ];
  return (
    <Modal title={t("usage.detail", { id: e.id })} onClose={onClose}>
      <dl className="grid grid-cols-[8rem_1fr] gap-x-4 gap-y-2 text-sm">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-slate-500">{k}</dt>
            <dd className="break-all">{v}</dd>
          </div>
        ))}
      </dl>
      {e.error && (
        <div>
          <div className="mb-1 text-sm text-slate-500">{t("usage.error_msg")}</div>
          <pre className="max-h-60 overflow-auto whitespace-pre-wrap break-all rounded-xl bg-red-50 p-3 text-xs text-red-700 dark:bg-red-950 dark:text-red-200">{e.error}</pre>
        </div>
      )}
    </Modal>
  );
}
