import { Database, RefreshCw, Search } from "lucide-react";
import { useState } from "react";
import { get } from "../api";
import { fmtNum, fmtTime } from "../format";
import { useI18n } from "../i18n";
import type { UsageEvent, UsagePage } from "../types";
import { Badge, Button, Card, ErrorBox, Modal, Pagination, inputBase, useLoad } from "../ui";
import StatsOverview from "./StatsOverview";

const PAGE_SIZE = 20;

export default function Usage() {
  const { t } = useI18n();
  const [filter, setFilter] = useState({ q: "", model: "", status: "", sort: "time_desc" });
  const [applied, setApplied] = useState(filter);
  const [page, setPage] = useState(1);
  const [detail, setDetail] = useState<UsageEvent | null>(null);
  const [gen, setGen] = useState(0);
  const usage = useLoad(() => {
    const qs = new URLSearchParams({ ...applied, page: String(page), page_size: String(PAGE_SIZE) });
    return get<UsagePage>(`/usage?${qs}`);
  }, [applied, page, gen]);

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h1 className="text-2xl font-bold tracking-tight">{t("nav.usage")}</h1>
        <Button icon={<RefreshCw className={`h-4 w-4 ${usage.loading ? "animate-spin" : ""}`} />} onClick={() => setGen(gen + 1)}>
          {t("common.refresh")}
        </Button>
      </div>
      <StatsOverview key={gen} />
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

function StatusPill({ status }: { status: number }) {
  return <Badge tone={status < 400 ? "green" : status === 429 ? "yellow" : "red"}>{status}</Badge>;
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
