import { Activity, Check, CircleDot, Copy, Pause, Play, RefreshCw, Search, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { get, post } from "../api";
import { fmtNum } from "../format";
import { useI18n } from "../i18n";
import { useToast } from "../toast";
import type { AccountsResponse, MonitorDetail, MonitorPage } from "../types";
import { Badge, Button, Card, ErrorBox, Modal, Segmented, Switch, inputBase, useAction, useLoad } from "../ui";

type StatusFilter = "" | "ok" | "error";

export default function Monitor() {
  const { t } = useI18n();
  const toast = useToast();
  const [status, setStatus] = useState<StatusFilter>("");
  const [q, setQ] = useState("");
  const [account, setAccount] = useState("");
  const [live, setLive] = useState(true);
  const [detail, setDetail] = useState<number | null>(null);
  const accounts = useLoad(() => get<AccountsResponse>("/accounts"));
  const page = useLoad(() => {
    const qs = new URLSearchParams({ status, q, limit: "300" });
    if (account) qs.set("account_id", account);
    return get<MonitorPage>(`/monitor?${qs}`);
  }, [status, q, account]);
  const act = useAction();

  // Poll while live and recording.
  const enabled = page.data?.enabled ?? false;
  useEffect(() => {
    if (!live || !enabled) return;
    const id = setInterval(() => void page.reload(), 2000);
    return () => clearInterval(id);
  }, [live, enabled, page.reload]);

  const setConfig = async (patch: Record<string, unknown>) => {
    const r = await act.run(() => post("/runtime", { monitor: patch }));
    if (r !== undefined) await page.reload();
  };

  const items = page.data?.items ?? [];
  const errors = items.filter((i) => i.status >= 400).length;
  const label = (id: number | null) => {
    if (id === null) return "—";
    const a = accounts.data?.accounts.find((x) => x.id === id);
    return a ? `#${id} ${a.email || a.label}` : `#${id}`;
  };

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <h1 className="text-2xl font-bold tracking-tight">{t("nav.monitor")}</h1>
          {enabled ? (
            <Badge tone="red" icon={<CircleDot className="h-3 w-3 animate-pulse" />}>
              {t("monitor.recording")}
            </Badge>
          ) : (
            <Badge>{t("monitor.paused")}</Badge>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <label className="flex items-center gap-2 text-sm">
            {t("monitor.record")}
            <Switch checked={enabled} disabled={act.busy || !page.data} onChange={(v) => setConfig({ enabled: v })} label={t("monitor.record")} />
          </label>
          <select
            className={inputBase}
            title={t("monitor.capacity")}
            value={page.data?.capacity ?? 500}
            onChange={(e) => setConfig({ capacity: Number(e.target.value) })}
          >
            {[100, 500, 1000, 5000].map((n) => (
              <option key={n} value={n}>
                {t("monitor.keep", { n })}
              </option>
            ))}
          </select>
          <Button icon={live ? <Pause className="h-4 w-4" /> : <Play className="h-4 w-4" />} onClick={() => setLive(!live)}>
            {live ? t("monitor.pause_live") : t("monitor.resume_live")}
          </Button>
          <Button icon={<RefreshCw className={`h-4 w-4 ${page.loading ? "animate-spin" : ""}`} />} onClick={() => void page.reload()}>
            {t("common.refresh")}
          </Button>
          <Button
            tone="danger"
            icon={<Trash2 className="h-4 w-4" />}
            disabled={!page.data?.held}
            onClick={async () => {
              const r = await act.run(() => post("/monitor/clear"));
              if (r !== undefined) toast("success", t("common.done"));
              await page.reload();
            }}
          >
            {t("monitor.clear")}
          </Button>
        </div>
      </div>
      <ErrorBox error={page.error ?? act.error} />

      {page.data && !enabled && page.data.held === 0 ? (
        <Card>
          <div className="flex flex-col items-center gap-3 py-10 text-center">
            <div className="flex h-14 w-14 items-center justify-center rounded-2xl bg-blue-50 dark:bg-blue-950">
              <Activity className="h-7 w-7 text-blue-500" />
            </div>
            <div className="text-lg font-semibold">{t("monitor.off_title")}</div>
            <p className="max-w-lg text-sm text-slate-500">{t("monitor.off_hint")}</p>
            <Button tone="primary" icon={<Play className="h-4 w-4" />} onClick={() => setConfig({ enabled: true })}>
              {t("monitor.start")}
            </Button>
          </div>
        </Card>
      ) : (
        <Card bodyClass="p-0">
          <div className="flex flex-wrap items-center gap-2 border-b border-slate-100 p-4 dark:border-slate-800">
            <Segmented
              value={status}
              onChange={setStatus}
              options={[
                { key: "", label: t("monitor.all") },
                { key: "ok", label: t("usage.ok") },
                { key: "error", label: t("usage.error") },
              ]}
            />
            <div className="relative min-w-48 flex-1 sm:max-w-xs">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-400" />
              <input className={`${inputBase} w-full pl-9`} placeholder={t("monitor.search")} value={q} onChange={(e) => setQ(e.target.value)} />
            </div>
            <select className={inputBase} value={account} onChange={(e) => setAccount(e.target.value)}>
              <option value="">{t("monitor.all_accounts")}</option>
              {accounts.data?.accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  #{a.id} {a.email || a.label}
                </option>
              ))}
            </select>
            <span className="ml-auto text-xs text-slate-500">
              {t("monitor.counts", { shown: items.length, held: page.data?.held ?? 0, cap: page.data?.capacity ?? 0, err: errors })}
            </span>
          </div>
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm">
              <thead className="bg-slate-50 text-xs uppercase tracking-wide text-slate-500 dark:bg-slate-950/50">
                <tr>
                  <th className="py-3 pl-4">{t("usage.status")}</th>
                  <th>{t("usage.time")}</th>
                  <th>{t("usage.model")}</th>
                  <th>{t("usage.user")}</th>
                  <th>{t("usage.account")}</th>
                  <th className="text-right">{t("monitor.tokens")}</th>
                  <th className="text-right">TTFT</th>
                  <th className="pr-4 text-right">{t("usage.latency")}</th>
                </tr>
              </thead>
              <tbody>
                {items.map((i) => (
                  <tr
                    key={i.id}
                    onClick={() => setDetail(i.id)}
                    className="cursor-pointer border-t border-slate-100 hover:bg-slate-50/60 dark:border-slate-800 dark:hover:bg-slate-800/30"
                  >
                    <td className="py-2.5 pl-4">
                      <div className="flex items-center gap-1">
                        <StatusPill status={i.status} />
                        {i.failover.length > 0 && (
                          <Badge tone="purple" title={t("monitor.failover")}>
                            ↻{i.failover.length}
                          </Badge>
                        )}
                      </div>
                    </td>
                    <td className="whitespace-nowrap font-mono text-xs text-slate-500">{new Date(i.ts_ms).toLocaleTimeString()}</td>
                    <td className="max-w-xs">
                      <div className="flex items-center gap-1.5">
                        <EndpointBadge endpoint={i.endpoint} />
                        {i.stream && <Badge>SSE</Badge>}
                        <span className="truncate font-mono text-xs">{i.model}</span>
                      </div>
                      {i.mapped_model !== i.model && <div className="truncate font-mono text-[11px] text-violet-500">→ {i.mapped_model}</div>}
                    </td>
                    <td className="whitespace-nowrap">{i.user_label}</td>
                    <td className="max-w-[12rem] truncate text-slate-500">{label(i.account_id)}</td>
                    <td className="whitespace-nowrap text-right tabular-nums">
                      {fmtNum(i.input_tokens)} / {fmtNum(i.output_tokens)}
                    </td>
                    <td className="text-right tabular-nums text-slate-500">{i.ttft_ms === null ? "—" : `${i.ttft_ms}ms`}</td>
                    <td className="pr-4 text-right tabular-nums">{(i.latency_ms / 1000).toFixed(2)}s</td>
                  </tr>
                ))}
                {items.length === 0 && (
                  <tr>
                    <td colSpan={8} className="py-10 text-center text-sm text-slate-400">
                      {enabled ? t("monitor.waiting") : t("usage.no_data")}
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </Card>
      )}
      {detail !== null && <DetailModal id={detail} label={label} onClose={() => setDetail(null)} />}
    </div>
  );
}

function StatusPill({ status }: { status: number }) {
  return <Badge tone={status < 400 ? "green" : status === 429 ? "yellow" : "red"}>{status}</Badge>;
}

function EndpointBadge({ endpoint }: { endpoint: string }) {
  return endpoint.includes("chat/completions") ? <Badge tone="blue">OpenAI</Badge> : <Badge tone="purple">Claude</Badge>;
}

type BodyTab = "request" | "upstream_request" | "response";

function DetailModal({ id, label, onClose }: { id: number; label: (id: number | null) => string; onClose: () => void }) {
  const { t } = useI18n();
  const d = useLoad(() => get<MonitorDetail>(`/monitor/${id}`), [id]);
  const [tab, setTab] = useState<BodyTab>("request");
  const e = d.data;
  const rows: [string, React.ReactNode][] = e
    ? [
        [t("usage.status"), <StatusPill key="s" status={e.status} />],
        [t("usage.time"), new Date(e.ts_ms).toLocaleString()],
        [t("monitor.endpoint"), <code key="e">{e.endpoint}</code>],
        [t("usage.model"), <code key="m">{e.model === e.mapped_model ? e.model : `${e.model} → ${e.mapped_model}`}</code>],
        [t("usage.user"), `${e.user_label} (${e.key_prefix}…)`],
        [t("monitor.ip"), e.client_ip || "—"],
        [t("usage.account"), label(e.account_id)],
        [t("monitor.tokens"), `${e.input_tokens.toLocaleString()} / ${e.output_tokens.toLocaleString()} · ${t("usage.cache_read")} ${e.cache_read_tokens.toLocaleString()} · ${t("usage.cache_write")} ${e.cache_creation_tokens.toLocaleString()}`],
        ["TTFT", e.ttft_ms === null ? "—" : `${e.ttft_ms} ms`],
        [t("usage.latency"), `${e.latency_ms.toLocaleString()} ms`],
      ]
    : [];
  return (
    <Modal title={t("usage.detail", { id })} onClose={onClose} wide>
      <ErrorBox error={d.error} />
      {e && (
        <>
          <dl className="grid gap-x-4 gap-y-2 text-sm sm:grid-cols-[8rem_1fr]">
            {rows.map(([k, v]) => (
              <div key={k} className="contents">
                <dt className="text-slate-500">{k}</dt>
                <dd className="break-all">{v}</dd>
              </div>
            ))}
          </dl>
          {e.error && <pre className="whitespace-pre-wrap break-all rounded-xl bg-red-50 p-3 text-xs text-red-700 dark:bg-red-950 dark:text-red-200">{e.error}</pre>}
          {e.failover.length > 0 && (
            <div>
              <div className="mb-1 text-sm font-medium">{t("monitor.failover")}</div>
              <div className="space-y-1">
                {e.failover.map((a, i) => (
                  <div key={i} className="flex items-center gap-2 rounded-lg bg-violet-50 px-3 py-1.5 text-xs dark:bg-violet-950/40">
                    <StatusPill status={a.status} />
                    <span>{label(a.account_id)}</span>
                    <span className="truncate text-slate-500">{a.error}</span>
                    <span className="ml-auto shrink-0 tabular-nums text-slate-400">{a.latency_ms}ms</span>
                  </div>
                ))}
              </div>
            </div>
          )}
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Segmented
              value={tab}
              onChange={setTab}
              options={[
                { key: "request", label: t("monitor.body_request") },
                { key: "upstream_request", label: t("monitor.body_upstream") },
                { key: "response", label: t("monitor.body_response") },
              ]}
            />
            {e.truncated && <Badge tone="yellow">{t("monitor.truncated")}</Badge>}
          </div>
          <BodyView text={e[tab]} />
        </>
      )}
    </Modal>
  );
}

function pretty(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

function BodyView({ text }: { text: string }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  const shown = pretty(text);
  return (
    <div className="relative">
      <pre className="max-h-[50vh] min-h-24 overflow-auto whitespace-pre-wrap break-all rounded-xl bg-slate-900 p-4 pr-20 text-xs leading-relaxed text-slate-100 dark:bg-slate-950">
        {shown || <span className="text-slate-500">{t("monitor.empty_body")}</span>}
      </pre>
      {shown && (
        <Button
          small
          className="absolute right-3 top-3"
          icon={copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
          onClick={async () => {
            await navigator.clipboard.writeText(shown).catch(() => {});
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          }}
        >
          {copied ? t("common.copied") : t("common.copy")}
        </Button>
      )}
    </div>
  );
}

