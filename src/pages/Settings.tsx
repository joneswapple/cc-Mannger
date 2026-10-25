import { ArrowRight, Info, Plus, RefreshCw, Route, Save, Server, Shuffle, Trash2, Boxes } from "lucide-react";
import { useEffect, useState } from "react";
import { get, isTauri, post } from "../api";
import { fmtTime } from "../format";
import { useI18n } from "../i18n";
import { useToast } from "../toast";
import type { AccountsResponse, ModelRoute, Settings as SettingsData, Status } from "../types";
import { Badge, Button, Card, ErrorBox, IconButton, Segmented, inputClass, useAction, useLoad } from "../ui";

type Tab = "models" | "routes" | "pool" | "upstream" | "about";

export default function Settings() {
  const { t } = useI18n();
  const { data, error, reload } = useLoad(() => get<SettingsData>("/settings"));
  const [tab, setTab] = useState<Tab>("models");
  const icons: Record<Tab, React.ReactNode> = {
    models: <Boxes className="h-4 w-4" />,
    routes: <Route className="h-4 w-4" />,
    pool: <Shuffle className="h-4 w-4" />,
    upstream: <Server className="h-4 w-4" />,
    about: <Info className="h-4 w-4" />,
  };

  return (
    <div className="space-y-5">
      <h1 className="text-2xl font-bold tracking-tight">{t("nav.settings")}</h1>
      <div className="overflow-x-auto">
        <Segmented value={tab} onChange={setTab} options={(Object.keys(icons) as Tab[]).map((k) => ({ key: k, label: <>{icons[k]}{t(`settings.tab_${k}`)}</> }))} />
      </div>
      <ErrorBox error={error} />
      {tab === "models" && data && <ModelsTab data={data} reload={reload} />}
      {tab === "routes" && data && <RoutesTab data={data} reload={reload} />}
      {tab === "pool" && <PoolTab />}
      {tab === "upstream" && data && <UpstreamTab data={data} />}
      {tab === "about" && <AboutTab />}
    </div>
  );
}

function ModelsTab({ data, reload }: { data: SettingsData; reload: () => Promise<void> }) {
  const { t } = useI18n();
  const toast = useToast();
  const [disabled, setDisabled] = useState<string[]>(data.disabled_models);
  const act = useAction();
  useEffect(() => setDisabled(data.disabled_models), [data]);
  const models = Array.from(new Set([...data.models.map((m) => m.id), ...disabled]));
  const names = Object.fromEntries(data.models.map((m) => [m.id, m.display_name]));

  return (
    <Card
      title={t("settings.models")}
      icon={<Boxes className="h-5 w-5 text-blue-500" />}
      actions={
        <>
          <Button
            icon={<RefreshCw className={`h-4 w-4 ${act.busy ? "animate-spin" : ""}`} />}
            disabled={act.busy}
            onClick={async () => {
              const r = await act.run(() => post("/settings/refresh-models"));
              if (r !== undefined) toast("success", t("common.done"));
              await reload();
            }}
          >
            {t("settings.refresh_catalog")}
          </Button>
          <Button
            tone="primary"
            icon={<Save className="h-4 w-4" />}
            disabled={act.busy}
            onClick={async () => {
              const r = await act.run(() => post("/settings/disabled-models", { models: disabled }));
              if (r !== undefined) toast("success", t("common.saved"));
              await reload();
            }}
          >
            {t("settings.save_denylist")}
          </Button>
        </>
      }
    >
      <ErrorBox error={act.error} />
      <p className="mb-1 text-xs text-slate-500">
        {t("settings.catalog_updated", { t: fmtTime(data.catalog.last_fetched) })}
        {data.catalog.last_error && <span className="text-red-600"> · {t("settings.catalog_error", { e: data.catalog.last_error })}</span>}
      </p>
      <p className="mb-3 text-xs text-slate-500">{t("settings.denylist_hint")}</p>
      <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
        {models.map((m) => {
          const off = disabled.includes(m);
          return (
            <label
              key={m}
              className={`flex cursor-pointer items-center gap-3 rounded-xl border p-3 transition-colors ${
                off ? "border-red-200 bg-red-50/50 dark:border-red-900 dark:bg-red-950/30" : "border-slate-200 hover:bg-slate-50 dark:border-slate-800 dark:hover:bg-slate-800/40"
              }`}
            >
              <input type="checkbox" checked={off} onChange={(e) => setDisabled(e.target.checked ? [...disabled, m] : disabled.filter((x) => x !== m))} />
              <div className="min-w-0">
                <div className="truncate text-sm font-medium">{names[m] || m}</div>
                <code className="block truncate text-xs text-slate-500">{m}</code>
              </div>
              <div className="ml-auto">
                {!names[m] ? <Badge tone="yellow">{t("settings.not_in_catalog")}</Badge> : off ? <Badge tone="red">{t("common.disabled")}</Badge> : null}
              </div>
            </label>
          );
        })}
        {models.length === 0 && <p className="text-sm text-slate-400">{t("usage.no_data")}</p>}
      </div>
    </Card>
  );
}

function RoutesTab({ data, reload }: { data: SettingsData; reload: () => Promise<void> }) {
  const { t } = useI18n();
  const toast = useToast();
  const [routes, setRoutes] = useState<ModelRoute[]>(data.model_routes ?? []);
  const act = useAction();
  useEffect(() => setRoutes(data.model_routes ?? []), [data]);
  const presets: ModelRoute[] = [
    { pattern: "re:^gpt-4o-mini", target: "claude-haiku-4-5" },
    { pattern: "re:^gpt-4", target: "claude-sonnet-4-5" },
    { pattern: "re:^o[13]", target: "claude-opus-4-1" },
  ];

  return (
    <Card
      title={t("settings.routes")}
      icon={<Route className="h-5 w-5 text-violet-500" />}
      actions={
        <>
          <Button onClick={() => setRoutes([...routes, ...presets.filter((p) => !routes.some((r) => r.pattern === p.pattern))])}>{t("settings.add_presets")}</Button>
          <Button icon={<Plus className="h-4 w-4" />} onClick={() => setRoutes([...routes, { pattern: "", target: "" }])}>
            {t("settings.add_route")}
          </Button>
          <Button
            tone="primary"
            icon={<Save className="h-4 w-4" />}
            disabled={act.busy}
            onClick={async () => {
              const r = await act.run(() => post("/settings/model-routes", { routes: routes.filter((r) => r.pattern.trim()) }));
              if (r !== undefined) toast("success", t("common.saved"));
              await reload();
            }}
          >
            {t("common.save")}
          </Button>
        </>
      }
    >
      <ErrorBox error={act.error} />
      <p className="mb-3 rounded-xl bg-slate-50 p-3 text-xs text-slate-500 dark:bg-slate-950/50">{t("settings.routes_hint")}</p>
      <datalist id="ccm-models">
        {data.models.map((m) => (
          <option key={m.id} value={m.id} />
        ))}
      </datalist>
      <div className="space-y-2">
        {routes.map((r, i) => (
          <div key={i} className="flex items-center gap-2">
            <span className="w-6 shrink-0 text-center text-xs text-slate-400">{i + 1}</span>
            <input
              className={`${inputClass} font-mono`}
              placeholder="gpt-4o / re:^gpt-"
              value={r.pattern}
              onChange={(e) => setRoutes(routes.map((x, j) => (j === i ? { ...x, pattern: e.target.value } : x)))}
            />
            <ArrowRight className="h-4 w-4 shrink-0 text-slate-400" />
            <input
              className={`${inputClass} font-mono`}
              placeholder="claude-sonnet-4-5"
              list="ccm-models"
              value={r.target}
              onChange={(e) => setRoutes(routes.map((x, j) => (j === i ? { ...x, target: e.target.value } : x)))}
            />
            <IconButton danger title={t("common.delete")} onClick={() => setRoutes(routes.filter((_, j) => j !== i))}>
              <Trash2 className="h-4 w-4" />
            </IconButton>
          </div>
        ))}
        {routes.length === 0 && <p className="py-6 text-center text-sm text-slate-400">{t("settings.no_routes")}</p>}
      </div>
    </Card>
  );
}

function PoolTab() {
  const { t } = useI18n();
  const acc = useLoad(() => get<AccountsResponse>("/accounts"));
  const p = acc.data?.pool;
  return (
    <Card title={t("settings.pool")} icon={<Shuffle className="h-5 w-5 text-emerald-500" />}>
      <ErrorBox error={acc.error} />
      <p className="mb-4 text-xs text-slate-500">{t("settings.pool_hint")}</p>
      {p && (
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          <Info2 label={t("settings.strategy")} value={t(`settings.strategy_${p.strategy}`)} sub={p.strategy} />
          <Info2 label={t("settings.sticky")} value={p.sticky ? t("common.yes") : t("common.no")} sub={p.sticky ? `TTL ${p.sticky_ttl_secs}s` : undefined} />
          <Info2 label={t("settings.max_retries")} value={String(p.max_retries)} />
          <Info2 label={t("settings.cooldown")} value={`${p.cooldown_secs}s`} />
          {p.quota_soft_limit !== undefined && <Info2 label={t("settings.soft_limit")} value={`${Math.round(p.quota_soft_limit * 100)}%`} />}
        </div>
      )}
    </Card>
  );
}

function Info2({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="rounded-xl border border-slate-200 p-4 dark:border-slate-800">
      <div className="text-xs text-slate-500">{label}</div>
      <div className="mt-1 text-lg font-semibold">{value}</div>
      {sub && <div className="font-mono text-xs text-slate-400">{sub}</div>}
    </div>
  );
}

function UpstreamTab({ data }: { data: SettingsData }) {
  const { t } = useI18n();
  const rows: [string, string][] = [
    [t("settings.source"), data.upstream.source],
    [t("settings.base_url"), data.upstream.base_url],
    ["anthropic-beta", data.upstream.anthropic_beta],
    ["OAuth token URL", data.upstream.oauth_token_url],
    ["OAuth client id", data.upstream.oauth_client_id],
    ["User-Agent", data.upstream.user_agent],
    [t("settings.inject"), data.upstream.inject_claude_code_system ? t("common.yes") : t("common.no")],
  ];
  return (
    <div className="grid gap-4 lg:grid-cols-3">
      <Card title={t("settings.upstream")} icon={<Server className="h-5 w-5 text-slate-500" />} className="lg:col-span-2">
        <dl className="divide-y divide-slate-100 text-sm dark:divide-slate-800">
          {rows.map(([k, v]) => (
            <div key={k} className="grid gap-1 py-2.5 sm:grid-cols-[12rem_1fr]">
              <dt className="text-slate-500">{k}</dt>
              <dd className="break-all font-mono text-xs">{v || "—"}</dd>
            </div>
          ))}
        </dl>
      </Card>
      <Card title={t("settings.effort")}>
        <div className="space-y-2">
          {Object.entries(data.upstream.effort_levels).map(([name, l]) => (
            <div key={name} className="flex items-center justify-between rounded-xl bg-slate-50 px-3 py-2 text-xs dark:bg-slate-950/50">
              <code className="font-semibold">{name}</code>
              <span className="text-slate-500">
                max {l.max_tokens}
                {l.thinking_budget ? ` · thinking ${l.thinking_budget}` : ""}
              </span>
            </div>
          ))}
        </div>
      </Card>
    </div>
  );
}

function AboutTab() {
  const { t } = useI18n();
  const status = useLoad(() => get<Status>("/status"));
  const s = status.data;
  return (
    <Card title={t("settings.tab_about")} icon={<Info className="h-5 w-5 text-blue-500" />}>
      <div className="flex flex-wrap items-center gap-4">
        <img src="/icon.png" alt="" className="h-16 w-16 rounded-2xl" onError={(e) => (e.currentTarget.style.display = "none")} />
        <div>
          <div className="text-xl font-bold">CC Manager</div>
          <div className="text-sm text-slate-500">{t("settings.about_desc")}</div>
          {s && (
            <div className="mt-2 flex flex-wrap gap-2">
              <Badge tone="blue">v{s.version}</Badge>
              <Badge>{isTauri ? t("settings.desktop") : t("settings.browser")}</Badge>
              <Badge tone="green">{s.listen}</Badge>
            </div>
          )}
        </div>
      </div>
      <p className="mt-4 text-xs text-slate-500">{t("settings.about_note")}</p>
    </Card>
  );
}
