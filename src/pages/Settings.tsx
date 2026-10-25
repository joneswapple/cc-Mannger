import { Boxes, Check, Cpu, ExternalLink, Info, Monitor as MonitorIcon, Network, Pause, Play, Plus, RefreshCw, RotateCcw, Save, ScrollText, Server, Settings2, Shuffle, Trash2, Wrench } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { get, isTauri, openExternal, post } from "../api";
import { fmtTime } from "../format";
import { useI18n } from "../i18n";
import { useTheme } from "../theme";
import { useToast } from "../toast";
import type { AccountsResponse, LogLine, ProxyEntry, ProxyTest, RuntimeView, Settings as SettingsData, SystemInfo, UpdateInfo } from "../types";
import { Badge, Button, Card, ErrorBox, Field, IconButton, Modal, Segmented, SettingRow, Switch, inputBase, inputClass, useAction, useLoad } from "../ui";

type Tab = "general" | "pool" | "proxies" | "models" | "upstream" | "advanced" | "logs" | "about";

export default function Settings() {
  const { t } = useI18n();
  const { data, error, reload } = useLoad(() => get<SettingsData>("/settings"));
  const [tab, setTab] = useState<Tab>(() => {
    const q = new URLSearchParams(location.hash.split("?")[1] ?? "").get("tab");
    return (q as Tab) || "general";
  });
  const icons: Record<Tab, React.ReactNode> = {
    general: <Settings2 className="h-4 w-4" />,
    pool: <Shuffle className="h-4 w-4" />,
    proxies: <Network className="h-4 w-4" />,
    models: <Boxes className="h-4 w-4" />,
    upstream: <Server className="h-4 w-4" />,
    advanced: <Wrench className="h-4 w-4" />,
    logs: <ScrollText className="h-4 w-4" />,
    about: <Info className="h-4 w-4" />,
  };

  return (
    <div className="space-y-5">
      <h1 className="text-2xl font-bold tracking-tight">{t("nav.settings")}</h1>
      <div className="overflow-x-auto">
        <Segmented value={tab} onChange={setTab} options={(Object.keys(icons) as Tab[]).map((k) => ({ key: k, label: <>{icons[k]}{t(`settings.tab_${k}`)}</> }))} />
      </div>
      <ErrorBox error={error} />
      {tab === "general" && <GeneralTab />}
      {tab === "pool" && <PoolTab />}
      {tab === "proxies" && <ProxiesTab />}
      {tab === "models" && data && <ModelsTab data={data} reload={reload} />}
      {tab === "upstream" && data && <UpstreamTab data={data} />}
      {tab === "advanced" && <AdvancedTab />}
      {tab === "logs" && <LogsTab />}
      {tab === "about" && <AboutTab />}
    </div>
  );
}

/// Loads runtime settings and saves patches with a toast.
function useRuntime() {
  const { t } = useI18n();
  const toast = useToast();
  const rt = useLoad(() => get<RuntimeView>("/runtime"));
  const act = useAction();
  const save = async (patch: Record<string, unknown>) => {
    const r = await act.run(() => post<RuntimeView>("/runtime", patch));
    if (r !== undefined) toast("success", t("common.saved"));
    await rt.reload();
  };
  return { rt, act, save };
}

function GeneralTab() {
  const { t, lang, toggle } = useI18n();
  const theme = useTheme();
  const { rt, act, save } = useRuntime();
  const [autostart, setAutostart] = useState<boolean | null>(null);
  useEffect(() => {
    if (isTauri) invoke<boolean>("desktop_autostart", { enable: null }).then(setAutostart, () => setAutostart(null));
  }, []);
  return (
    <Card title={t("settings.tab_general")} icon={<Settings2 className="h-5 w-5 text-blue-500" />}>
      <ErrorBox error={act.error ?? rt.error} />
      <div className="divide-y divide-slate-100 dark:divide-slate-800">
        <SettingRow title={t("settings.language")}>
          <Segmented value={lang} onChange={(v) => v !== lang && toggle()} options={[{ key: "zh", label: "中文" }, { key: "en", label: "English" }]} />
        </SettingRow>
        <SettingRow title={t("settings.theme")}>
          <Segmented
            value={theme.pref}
            onChange={theme.setPref}
            options={[
              { key: "light", label: t("nav.theme_light") },
              { key: "dark", label: t("nav.theme_dark") },
              { key: "system", label: t("nav.theme_system") },
            ]}
          />
        </SettingRow>
        {isTauri ? (
          <>
            <SettingRow title={t("settings.launch_at_login")} hint={t("settings.launch_at_login_hint")}>
              <Switch
                checked={!!autostart}
                disabled={autostart === null}
                onChange={async (v) => {
                  try {
                    setAutostart(await invoke<boolean>("desktop_autostart", { enable: v }));
                  } catch (e) {
                    act.setError(String(e));
                  }
                }}
              />
            </SettingRow>
            {rt.data && (
              <SettingRow title={t("proxy.auto_start")} hint={t("proxy.auto_start_hint")}>
                <Switch checked={rt.data.effective.desktop.auto_start_proxy} onChange={(v) => save({ desktop: { auto_start_proxy: v } })} />
              </SettingRow>
            )}
            <SettingRow title={t("settings.mini")} hint={t("settings.mini_hint")}>
              <Button icon={<MonitorIcon className="h-4 w-4" />} onClick={() => invoke("open_mini").catch((e) => act.setError(String(e)))}>
                {t("settings.open_mini")}
              </Button>
            </SettingRow>
          </>
        ) : (
          <p className="py-3 text-xs text-slate-500">{t("settings.desktop_only")}</p>
        )}
      </div>
    </Card>
  );
}

function PoolTab() {
  const { t } = useI18n();
  const { rt, act, save } = useRuntime();
  const p = rt.data?.effective.pool;
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [poll, setPoll] = useState("");
  useEffect(() => {
    if (p) {
      setDraft({
        sticky_ttl_secs: String(p.sticky_ttl_secs),
        max_retries: String(p.max_retries),
        cooldown_secs: String(p.cooldown_secs),
        quota_soft_limit: String(Math.round(p.quota_soft_limit * 100)),
      });
      setPoll(String(rt.data?.effective.usage_poll_secs ?? 600));
    }
  }, [p, rt.data]);
  if (!p) return <ErrorBox error={rt.error} />;
  const overridden = Object.keys((rt.data?.overrides.pool as Record<string, unknown>) ?? {});
  const num = (k: string) => Number(draft[k] ?? 0);
  const numInput = (k: string, w = "w-24") => (
    <input className={`${inputBase} ${w} tabular-nums`} inputMode="numeric" value={draft[k] ?? ""} onChange={(e) => setDraft({ ...draft, [k]: e.target.value.replace(/[^\d]/g, "") })} />
  );
  const mark = (k: string) => (overridden.includes(k) ? <Badge tone="blue">{t("settings.overridden")}</Badge> : null);
  return (
    <Card
      title={t("settings.pool")}
      icon={<Shuffle className="h-5 w-5 text-emerald-500" />}
      actions={
        <>
          <Button icon={<RotateCcw className="h-4 w-4" />} disabled={overridden.length === 0} onClick={() => save({ pool: null })}>
            {t("settings.reset_section")}
          </Button>
          <Button
            tone="primary"
            icon={<Save className="h-4 w-4" />}
            disabled={act.busy}
            onClick={() =>
              save({
                pool: { sticky_ttl_secs: num("sticky_ttl_secs"), max_retries: num("max_retries"), cooldown_secs: num("cooldown_secs"), quota_soft_limit: num("quota_soft_limit") / 100 },
                usage_poll_secs: Number(poll) || 600,
              })
            }
          >
            {t("common.save")}
          </Button>
        </>
      }
    >
      <ErrorBox error={act.error} />
      <p className="mb-2 text-xs text-slate-500">{t("settings.pool_live_hint")}</p>
      <div className="divide-y divide-slate-100 dark:divide-slate-800">
        <SettingRow title={<>{t("settings.strategy")} {mark("strategy")}</>} hint={t(`settings.strategy_${p.strategy}_hint`)}>
          <Segmented
            value={p.strategy}
            onChange={(v) => save({ pool: { strategy: v } })}
            options={(["priority", "round_robin", "least_used"] as const).map((k) => ({ key: k, label: t(`settings.strategy_${k}`) }))}
          />
        </SettingRow>
        <SettingRow title={<>{t("settings.sticky")} {mark("sticky")}</>} hint={t("settings.sticky_hint")}>
          <Switch checked={p.sticky} onChange={(v) => save({ pool: { sticky: v } })} />
        </SettingRow>
        <SettingRow title={<>{t("settings.sticky_ttl")} {mark("sticky_ttl_secs")}</>} hint={t("settings.seconds")}>
          {numInput("sticky_ttl_secs")}
        </SettingRow>
        <SettingRow title={<>{t("settings.max_retries")} {mark("max_retries")}</>} hint={t("settings.max_retries_hint")}>
          {numInput("max_retries", "w-16")}
        </SettingRow>
        <SettingRow title={<>{t("settings.cooldown")} {mark("cooldown_secs")}</>} hint={t("settings.cooldown_hint")}>
          {numInput("cooldown_secs")}
        </SettingRow>
        <SettingRow title={<>{t("settings.soft_limit")} {mark("quota_soft_limit")}</>} hint={t("settings.soft_limit_hint")}>
          {numInput("quota_soft_limit", "w-16")}
          <span className="text-sm text-slate-500">%</span>
        </SettingRow>
        <SettingRow title={t("settings.usage_poll")} hint={t("settings.usage_poll_hint")}>
          <input className={`${inputBase} w-24 tabular-nums`} inputMode="numeric" value={poll} onChange={(e) => setPoll(e.target.value.replace(/[^\d]/g, ""))} />
        </SettingRow>
      </div>
    </Card>
  );
}

function ProxiesTab() {
  const { t } = useI18n();
  const toast = useToast();
  const list = useLoad(() => get<{ proxies: ProxyEntry[] }>("/proxies"));
  const accounts = useLoad(() => get<AccountsResponse>("/accounts"));
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [tests, setTests] = useState<Record<string, ProxyTest | "running">>({});
  const [assign, setAssign] = useState<ProxyEntry | null>(null);
  const act = useAction();
  const test = async (key: string, run: () => Promise<ProxyTest>) => {
    setTests((x) => ({ ...x, [key]: "running" }));
    try {
      const r = await run();
      setTests((x) => ({ ...x, [key]: r }));
    } catch (e) {
      setTests((x) => ({ ...x, [key]: { ok: false, latency_ms: 0, status: null, error: String(e) } }));
    }
  };
  const label = (id: number) => {
    const a = accounts.data?.accounts.find((x) => x.id === id);
    return a ? `#${id} ${a.email || a.label}` : `#${id}`;
  };
  const Result = ({ r }: { r?: ProxyTest | "running" }) =>
    !r ? null : r === "running" ? <RefreshCw className="h-4 w-4 animate-spin text-slate-400" /> : r.ok ? <Badge tone="green">{t("settings.proxy_ok", { ms: r.latency_ms })}</Badge> : <Badge tone="red" title={r.error}>{t("settings.proxy_fail")}</Badge>;
  return (
    <div className="space-y-4">
      <Card title={t("settings.proxy_add")} icon={<Plus className="h-5 w-5 text-blue-500" />}>
        <ErrorBox error={act.error ?? list.error} />
        <div className="flex flex-wrap items-end gap-2">
          <div className="w-44">
            <Field label={t("settings.proxy_name")}>
              <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} placeholder="hk-1" />
            </Field>
          </div>
          <div className="min-w-56 flex-1">
            <Field label={t("settings.proxy_url")}>
              <input className={`${inputClass} font-mono`} value={url} onChange={(e) => setUrl(e.target.value)} placeholder="socks5h://user:pass@host:1080" />
            </Field>
          </div>
          <Button disabled={!url.trim()} onClick={() => test("new", () => post<ProxyTest>("/proxies/test", { url }))}>
            {t("proxy.test")}
          </Button>
          <Button
            tone="primary"
            icon={<Plus className="h-4 w-4" />}
            disabled={act.busy || !url.trim()}
            onClick={async () => {
              const r = await act.run(() => post("/proxies", { name, url }));
              if (r !== undefined) {
                toast("success", t("common.done"));
                setName("");
                setUrl("");
                await list.reload();
              }
            }}
          >
            {t("security.add")}
          </Button>
          <Result r={tests.new} />
        </div>
        <p className="mt-3 text-xs text-slate-500">{t("settings.proxy_hint")}</p>
      </Card>
      <Card bodyClass="p-0">
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead className="bg-slate-50 text-xs uppercase tracking-wide text-slate-500 dark:bg-slate-950/50">
              <tr>
                <th className="py-3 pl-4">{t("settings.proxy_name")}</th>
                <th>{t("settings.proxy_url")}</th>
                <th>{t("settings.proxy_accounts")}</th>
                <th />
                <th className="pr-4" />
              </tr>
            </thead>
            <tbody>
              {list.data?.proxies.map((p) => (
                <tr key={p.id} className="border-t border-slate-100 dark:border-slate-800">
                  <td className="py-2.5 pl-4 font-medium">{p.name}</td>
                  <td className="max-w-xs truncate font-mono text-xs">{p.url}</td>
                  <td className="max-w-sm">
                    <div className="flex flex-wrap gap-1">{p.accounts.length ? p.accounts.map((a) => <Badge key={a}>{label(a)}</Badge>) : <span className="text-xs text-slate-400">—</span>}</div>
                  </td>
                  <td>
                    <Result r={tests[p.id]} />
                  </td>
                  <td className="whitespace-nowrap pr-4 text-right">
                    <Button small onClick={() => test(String(p.id), () => post<ProxyTest>(`/proxies/${p.id}/test`))}>
                      {t("proxy.test")}
                    </Button>{" "}
                    <Button small onClick={() => setAssign(p)}>
                      {t("settings.proxy_assign")}
                    </Button>
                    <IconButton
                      danger
                      title={t("common.delete")}
                      onClick={async () => {
                        const r = await act.run(() => post(`/proxies/${p.id}/delete`));
                        if (r !== undefined) await list.reload();
                      }}
                    >
                      <Trash2 className="h-4 w-4" />
                    </IconButton>
                  </td>
                </tr>
              ))}
              {list.data?.proxies.length === 0 && (
                <tr>
                  <td colSpan={5} className="py-10 text-center text-sm text-slate-400">
                    {t("settings.no_proxies")}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </Card>
      {assign && accounts.data && (
        <AssignModal
          proxy={assign}
          accounts={accounts.data}
          onClose={() => setAssign(null)}
          onDone={async () => {
            await list.reload();
            await accounts.reload();
          }}
        />
      )}
    </div>
  );
}

function AssignModal({ proxy, accounts, onClose, onDone }: { proxy: ProxyEntry; accounts: AccountsResponse; onClose: () => void; onDone: () => Promise<void> }) {
  const { t } = useI18n();
  const toast = useToast();
  const [sel, setSel] = useState<number[]>(proxy.accounts);
  const act = useAction();
  return (
    <Modal title={t("settings.proxy_assign_title", { name: proxy.name })} onClose={onClose}>
      <ErrorBox error={act.error} />
      <p className="text-xs text-slate-500">{t("settings.proxy_assign_hint")}</p>
      <div className="max-h-72 space-y-1 overflow-y-auto">
        {accounts.accounts.map((a) => (
          <label key={a.id} className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm hover:bg-slate-50 dark:hover:bg-slate-800/50">
            <input type="checkbox" checked={sel.includes(a.id)} onChange={(e) => setSel(e.target.checked ? [...sel, a.id] : sel.filter((x) => x !== a.id))} />
            <span>
              #{a.id} {a.email || a.label}
            </span>
            {a.proxy_url && a.proxy_url !== proxy.url && <Badge tone="yellow">{t("settings.proxy_other")}</Badge>}
          </label>
        ))}
      </div>
      <div className="flex justify-end gap-2">
        <Button onClick={onClose}>{t("common.cancel")}</Button>
        <Button
          tone="primary"
          icon={<Check className="h-4 w-4" />}
          disabled={act.busy}
          onClick={async () => {
            const r = await act.run(() => post(`/proxies/${proxy.id}/assign`, { account_ids: sel }));
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


function AdvancedTab() {
  const { t } = useI18n();
  const sys = useLoad(() => get<SystemInfo>("/system"));
  const { rt, act, save } = useRuntime();
  const s = sys.data;
  const overrides = rt.data?.overrides ?? {};
  const rows: [string, string][] = s
    ? [
        [t("settings.version"), s.version],
        [t("settings.platform"), `${s.os} / ${s.arch}`],
        [t("settings.data_dir"), s.data_dir],
        [t("settings.database"), s.database],
        [t("settings.log_level"), s.log_level],
      ]
    : [];
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card title={t("settings.system")} icon={<Cpu className="h-5 w-5 text-slate-500" />}>
        <ErrorBox error={sys.error} />
        <dl className="divide-y divide-slate-100 text-sm dark:divide-slate-800">
          {rows.map(([k, v]) => (
            <div key={k} className="grid gap-1 py-2.5 sm:grid-cols-[9rem_1fr]">
              <dt className="text-slate-500">{k}</dt>
              <dd className="break-all font-mono text-xs">{v}</dd>
            </div>
          ))}
        </dl>
      </Card>
      <Card
        title={t("settings.overrides")}
        icon={<Wrench className="h-5 w-5 text-violet-500" />}
        actions={
          <Button
            tone="danger"
            icon={<RotateCcw className="h-4 w-4" />}
            disabled={Object.keys(overrides).length === 0 || act.busy}
            onClick={() => {
              if (confirm(t("settings.reset_all_confirm"))) void save(Object.fromEntries(Object.keys(overrides).map((k) => [k, null])));
            }}
          >
            {t("settings.reset_all")}
          </Button>
        }
      >
        <ErrorBox error={act.error} />
        <p className="mb-2 text-xs text-slate-500">{t("settings.overrides_hint")}</p>
        <pre className="max-h-80 overflow-auto rounded-xl bg-slate-900 p-4 text-xs text-slate-100 dark:bg-slate-950">{JSON.stringify(overrides, null, 2)}</pre>
      </Card>
    </div>
  );
}

const LEVELS = ["ERROR", "WARN", "INFO", "DEBUG"] as const;

function LogsTab() {
  const { t } = useI18n();
  const [level, setLevel] = useState<(typeof LEVELS)[number]>("INFO");
  const [lines, setLines] = useState<LogLine[]>([]);
  const [live, setLive] = useState(true);
  const [q, setQ] = useState("");
  const [error, setError] = useState<string | null>(null);
  const last = useRef(0);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    last.current = 0;
    setLines([]);
  }, [level]);
  useEffect(() => {
    let stop = false;
    const tick = async () => {
      try {
        const r = await get<{ lines: LogLine[] }>(`/logs?after=${last.current}&level=${level}&limit=500`);
        if (stop) return;
        if (r.lines.length) {
          last.current = r.lines[r.lines.length - 1].seq;
          setLines((x) => [...x, ...r.lines].slice(-2000));
        }
        setError(null);
      } catch (e) {
        if (!stop) setError(String(e));
      }
    };
    void tick();
    if (!live) return () => void (stop = true);
    const id = setInterval(tick, 2000);
    return () => {
      stop = true;
      clearInterval(id);
    };
  }, [level, live]);
  useEffect(() => {
    if (live && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [lines, live]);
  const shown = q.trim() ? lines.filter((l) => `${l.target} ${l.message}`.toLowerCase().includes(q.trim().toLowerCase())) : lines;
  const color: Record<string, string> = { ERROR: "text-red-400", WARN: "text-amber-300", INFO: "text-sky-300", DEBUG: "text-slate-400", TRACE: "text-slate-500" };
  return (
    <Card
      title={t("settings.tab_logs")}
      icon={<ScrollText className="h-5 w-5 text-slate-500" />}
      actions={
        <>
          <Segmented value={level} onChange={setLevel} options={LEVELS.map((l) => ({ key: l, label: l }))} />
          <input className={`${inputBase} w-44`} placeholder={t("common.search")} value={q} onChange={(e) => setQ(e.target.value)} />
          <Button icon={live ? <Pause className="h-4 w-4" /> : <Play className="h-4 w-4" />} onClick={() => setLive(!live)}>
            {live ? t("monitor.pause_live") : t("monitor.resume_live")}
          </Button>
          <Button
            tone="danger"
            icon={<Trash2 className="h-4 w-4" />}
            onClick={async () => {
              await post("/logs/clear").catch((e) => setError(String(e)));
              setLines([]);
            }}
          >
            {t("monitor.clear")}
          </Button>
        </>
      }
    >
      <ErrorBox error={error} />
      <div ref={box} className="h-[60vh] overflow-auto rounded-xl bg-slate-950 p-3 font-mono text-[11px] leading-5 text-slate-200">
        {shown.map((l) => (
          <div key={l.seq} className="whitespace-pre-wrap break-all">
            <span className="text-slate-500">{new Date(l.ts_ms).toLocaleTimeString()}</span> <span className={`${color[l.level] ?? ""} inline-block w-12`}>{l.level}</span>{" "}
            <span className="text-slate-500">{l.target}</span> {l.message}
          </div>
        ))}
        {shown.length === 0 && <div className="text-slate-500">{t("settings.no_logs")}</div>}
      </div>
    </Card>
  );
}

function AboutTab() {
  const { t } = useI18n();
  const sys = useLoad(() => get<SystemInfo>("/system"));
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const act = useAction();
  const s = sys.data;
  return (
    <Card title={t("settings.tab_about")} icon={<Info className="h-5 w-5 text-blue-500" />}>
      <div className="flex flex-wrap items-center gap-4">
        <img src="/icon.png" alt="" className="h-16 w-16 rounded-2xl" onError={(e) => (e.currentTarget.style.display = "none")} />
        <div className="min-w-0 flex-1">
          <div className="text-xl font-bold">CC Manager</div>
          <div className="text-sm text-slate-500">{t("settings.about_desc")}</div>
          {s && (
            <div className="mt-2 flex flex-wrap gap-2">
              <Badge tone="blue">v{s.version}</Badge>
              <Badge>{s.desktop ? t("settings.desktop") : t("settings.browser")}</Badge>
              <Badge>
                {s.os} / {s.arch}
              </Badge>
            </div>
          )}
        </div>
        <Button
          icon={<RefreshCw className={`h-4 w-4 ${act.busy ? "animate-spin" : ""}`} />}
          disabled={act.busy || s?.update_check === false}
          onClick={async () => {
            const r = await act.run(() => get<UpdateInfo>("/update/check"));
            if (r) setUpdate(r);
          }}
        >
          {t("settings.check_update")}
        </Button>
      </div>
      <ErrorBox error={act.error} />
      {update && (
        <div className={`mt-4 rounded-xl border p-4 text-sm ${update.newer ? "border-blue-200 bg-blue-50/60 dark:border-blue-900 dark:bg-blue-950/40" : "border-slate-200 dark:border-slate-800"}`}>
          {update.newer ? (
            <div className="space-y-2">
              <div className="font-medium">{t("settings.update_available", { v: update.latest })}</div>
              {update.notes && <pre className="max-h-40 overflow-auto whitespace-pre-wrap text-xs text-slate-600 dark:text-slate-300">{update.notes}</pre>}
              <Button tone="primary" icon={<ExternalLink className="h-4 w-4" />} onClick={() => openExternal(update.url)}>
                {t("settings.open_release")}
              </Button>
            </div>
          ) : (
            <div className="flex items-center gap-2 text-emerald-600">
              <Check className="h-4 w-4" /> {t("settings.up_to_date", { v: update.current })}
            </div>
          )}
        </div>
      )}
      <p className="mt-4 text-xs text-slate-500">{t("settings.about_note")}</p>
    </Card>
  );
}
