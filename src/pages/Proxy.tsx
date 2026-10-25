import { ArrowRight, Check, Cloud, Copy, FlaskConical, Globe, KeyRound, Link2, Play, Plus, Radio, RefreshCw, RotateCcw, Route, Save, Square, Terminal, Trash2, Undo2, Wand2 } from "lucide-react";
import { useEffect, useState } from "react";
import { get, isTauri, post } from "../api";
import { useI18n } from "../i18n";
import TunnelCard from "./TunnelCard";
import { useToast } from "../toast";
import type { ClaudeCodeStatus, ModelRoute, ModelTest, RuntimeView, ServiceStatus, Settings } from "../types";
import { Badge, Button, Card, ErrorBox, Field, IconButton, Segmented, SettingRow, Switch, inputBase, inputClass, useAction, useLoad } from "../ui";

type Snip = "shell" | "powershell" | "settings" | "anthropic" | "openai" | "curl";

/// Family rules are ordinary model routes with these exact patterns; the
/// page shows them as three selects and keeps them after the custom rules.
const FAMILIES = [
  { key: "opus", pattern: "re:(?i)opus" },
  { key: "sonnet", pattern: "re:(?i)sonnet" },
  { key: "haiku", pattern: "re:(?i)haiku" },
] as const;

const OPENAI_PRESETS: ModelRoute[] = [
  { pattern: "re:^gpt-4o-mini", target: "claude-haiku-4-5" },
  { pattern: "re:^gpt-", target: "claude-sonnet-4-5" },
  { pattern: "re:^o[134]", target: "claude-opus-4-1" },
];

function splitListen(listen: string): { host: string; port: string } {
  const i = listen.lastIndexOf(":");
  return i < 0 ? { host: listen, port: "" } : { host: listen.slice(0, i), port: listen.slice(i + 1) };
}

export default function Proxy() {
  const { t } = useI18n();
  const service = useLoad(() => get<ServiceStatus>("/service"));
  const runtime = useLoad(() => get<RuntimeView>("/runtime"));
  const settings = useLoad(() => get<Settings>("/settings"));
  const [publicUrl, setPublicUrl] = useState<string | null>(null);
  const port = splitListen(service.data?.bound ?? service.data?.listen ?? ":8787").port || "8787";
  const reloadAll = () => {
    void service.reload();
    void runtime.reload();
    void settings.reload();
  };

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h1 className="text-2xl font-bold tracking-tight">{t("nav.proxy")}</h1>
        <Button icon={<RefreshCw className={`h-4 w-4 ${service.loading ? "animate-spin" : ""}`} />} onClick={reloadAll}>
          {t("common.refresh")}
        </Button>
      </div>
      <ErrorBox error={service.error ?? runtime.error ?? settings.error} />
      {service.data && runtime.data && <ServiceCard svc={service.data} rt={runtime.data} reload={reloadAll} />}
      <TunnelCard onUrl={setPublicUrl} />
      <Connection port={port} settings={settings.data} publicUrl={publicUrl} />
      {settings.data && runtime.data && <Mapping settings={settings.data} rt={runtime.data} reload={reloadAll} />}
      {settings.data && <ModelTests settings={settings.data} />}
      <ClaudeCodeSync port={port} />
    </div>
  );
}

function ServiceCard({ svc, rt, reload }: { svc: ServiceStatus; rt: RuntimeView; reload: () => void }) {
  const { t } = useI18n();
  const toast = useToast();
  const act = useAction();
  const cur = splitListen(rt.effective.listen);
  const [host, setHost] = useState(cur.host);
  const [port, setPort] = useState(cur.port);
  useEffect(() => {
    setHost(cur.host);
    setPort(cur.port);
  }, [cur.host, cur.port]);
  const lan = host === "0.0.0.0" || host === "[::]";
  const dirty = `${host}:${port}` !== rt.effective.listen;

  const action = async (a: "start" | "stop" | "restart") => {
    const r = await act.run(() => post(`/service/${a}`));
    if (r !== undefined) toast("success", t({ start: "proxy.started", stop: "proxy.stopped_ok", restart: "proxy.restarted" }[a]));
    reload();
  };

  return (
    <Card bodyClass="p-5">
      <ErrorBox error={act.error} />
      <div className="flex flex-wrap items-center gap-4">
        <div className={`flex h-12 w-12 items-center justify-center rounded-2xl ${svc.running ? "bg-emerald-50 dark:bg-emerald-950" : "bg-slate-100 dark:bg-slate-800"}`}>
          <Radio className={`h-6 w-6 ${svc.running ? "text-emerald-500" : "text-slate-400"}`} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-lg font-semibold">{t("connect.service")}</span>
            {svc.running ? <Badge tone="green">● {t("connect.running")}</Badge> : <Badge tone="red">{t("proxy.stopped")}</Badge>}
            {svc.restart_required && <Badge tone="yellow">{t("proxy.restart_required")}</Badge>}
          </div>
          <div className="mt-0.5 font-mono text-sm text-slate-500">{svc.bound ?? svc.listen}</div>
        </div>
        {svc.controllable && (
          <div className="flex flex-wrap gap-2">
            {svc.running ? (
              <>
                <Button icon={<RotateCcw className="h-4 w-4" />} disabled={act.busy} onClick={() => action("restart")}>
                  {t("proxy.restart")}
                </Button>
                <Button tone="danger" icon={<Square className="h-4 w-4" />} disabled={act.busy} onClick={() => action("stop")}>
                  {t("proxy.stop")}
                </Button>
              </>
            ) : (
              <Button tone="primary" icon={<Play className="h-4 w-4" />} disabled={act.busy} onClick={() => action("start")}>
                {t("proxy.start")}
              </Button>
            )}
          </div>
        )}
      </div>
      <div className="mt-4 divide-y divide-slate-100 border-t border-slate-100 dark:divide-slate-800 dark:border-slate-800">
        <SettingRow title={t("proxy.lan")} hint={t("proxy.lan_hint")}>
          <Switch checked={lan} onChange={(v) => setHost(v ? "0.0.0.0" : "127.0.0.1")} label={t("proxy.lan")} />
        </SettingRow>
        <SettingRow title={t("proxy.port")} hint={svc.controllable ? t("proxy.port_hint_desktop") : t("proxy.port_hint_server")}>
          <input className={`${inputBase} w-28 font-mono`} inputMode="numeric" value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))} />
          <Button
            tone="primary"
            icon={<Save className="h-4 w-4" />}
            disabled={!dirty || !port || act.busy}
            onClick={async () => {
              const r = await act.run(() => post("/runtime", { listen: `${host}:${port}` }));
              if (r === undefined) return;
              if (svc.controllable && svc.running) {
                await act.run(() => post("/service/restart"));
                toast("success", t("proxy.restarted"));
              } else {
                toast("info", t("proxy.saved_restart"));
              }
              reload();
            }}
          >
            {t("common.save")}
          </Button>
        </SettingRow>
        {svc.controllable && (
          <SettingRow title={t("proxy.auto_start")} hint={t("proxy.auto_start_hint")}>
            <Switch
              checked={rt.effective.desktop.auto_start_proxy}
              onChange={async (v) => {
                await act.run(() => post("/runtime", { desktop: { auto_start_proxy: v } }));
                reload();
              }}
            />
          </SettingRow>
        )}
      </div>
    </Card>
  );
}

function Connection({ port, settings, publicUrl }: { port: string; settings: Settings | null; publicUrl: string | null }) {
  const { t } = useI18n();
  const defaultUrl = isTauri ? `http://${t("connect.lan_ip")}:${port}` : location.origin;
  const [url, setUrl] = useState(defaultUrl);
  const [key, setKey] = useState("sk-cp-...");
  const [snip, setSnip] = useState<Snip>("shell");
  useEffect(() => setUrl(defaultUrl), [defaultUrl]);
  const models = (settings?.models ?? []).filter((m) => !settings?.disabled_models.includes(m.id));
  const model = models.find((m) => m.id.includes("sonnet"))?.id ?? models[0]?.id ?? "claude-sonnet-4-5";

  const snippets: Record<Snip, { label: string; text: string }> = {
    shell: { label: "macOS / Linux", text: `export ANTHROPIC_BASE_URL=${url}\nexport ANTHROPIC_AUTH_TOKEN=${key}\nclaude` },
    powershell: { label: "PowerShell", text: `$env:ANTHROPIC_BASE_URL = "${url}"\n$env:ANTHROPIC_AUTH_TOKEN = "${key}"\nclaude` },
    settings: { label: "settings.json", text: `// ~/.claude/settings.json\n${JSON.stringify({ env: { ANTHROPIC_BASE_URL: url, ANTHROPIC_AUTH_TOKEN: key } }, null, 2)}` },
    anthropic: {
      label: "Anthropic SDK",
      text: `import anthropic\n\nclient = anthropic.Anthropic(base_url="${url}", api_key="${key}")\nmsg = client.messages.create(\n    model="${model}", max_tokens=1024,\n    messages=[{"role": "user", "content": "hi"}],\n)\nprint(msg.content[0].text)`,
    },
    openai: {
      label: "OpenAI SDK",
      text: `from openai import OpenAI\n\nclient = OpenAI(base_url="${url}/v1", api_key="${key}")\nresp = client.chat.completions.create(\n    model="${model}",\n    messages=[{"role": "user", "content": "hi"}],\n)\nprint(resp.choices[0].message.content)`,
    },
    curl: {
      label: "cURL",
      text: `curl ${url}/v1/messages \\\n  -H "x-api-key: ${key}" \\\n  -H "anthropic-version: 2023-06-01" \\\n  -H "content-type: application/json" \\\n  -d '{"model":"${model}","max_tokens":256,"messages":[{"role":"user","content":"hi"}]}'`,
    },
  };

  const endpoints: [string, string, string][] = [
    ["POST", "/v1/messages", t("connect.ep_messages")],
    ["POST", "/v1/messages/count_tokens", t("connect.ep_count")],
    ["POST", "/v1/chat/completions", t("connect.ep_openai")],
    ["GET", "/v1/models", t("connect.ep_models")],
    ["POST", "/register", t("connect.ep_register")],
    ["GET", "/metrics", t("connect.ep_metrics")],
  ];

  return (
    <div className="grid gap-4 lg:grid-cols-5">
      <Card title={t("connect.params")} icon={<Link2 className="h-5 w-5 text-blue-500" />} className="min-w-0 lg:col-span-2">
        <div className="space-y-3">
          <Field label={t("connect.url")} hint={t("connect.url_hint")}>
            <div className="relative">
              <Globe className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-400" />
              <input className={`${inputClass} pl-9 font-mono`} value={url} onChange={(e) => setUrl(e.target.value)} />
            </div>
          </Field>
          {publicUrl && url !== publicUrl && (
            <Button small icon={<Cloud className="h-3.5 w-3.5" />} onClick={() => setUrl(publicUrl)}>
              {t("tunnel.use_public")}
            </Button>
          )}
          <Field label={t("connect.key")} hint={t("connect.key_hint")}>
            <div className="relative">
              <KeyRound className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-400" />
              <input className={`${inputClass} pl-9 font-mono`} value={key} onChange={(e) => setKey(e.target.value)} />
            </div>
          </Field>
          <div>
            <div className="mb-1 text-sm font-medium text-slate-600 dark:text-slate-300">{t("connect.endpoints")}</div>
            <div className="divide-y divide-slate-100 rounded-xl border border-slate-200 text-xs dark:divide-slate-800 dark:border-slate-800">
              {endpoints.map(([m, p, d]) => (
                <div key={p} className="flex items-center gap-2 px-3 py-2">
                  <span className={`w-10 shrink-0 font-mono font-semibold ${m === "GET" ? "text-emerald-600" : "text-blue-600"}`}>{m}</span>
                  <code className="truncate">{p}</code>
                  <span className="ml-auto shrink-0 text-slate-400">{d}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      </Card>
      <Card title={t("connect.snippets")} icon={<Terminal className="h-5 w-5 text-violet-500" />} className="min-w-0 lg:col-span-3">
        <div className="mb-3 overflow-x-auto">
          <Segmented value={snip} onChange={setSnip} options={(Object.keys(snippets) as Snip[]).map((k) => ({ key: k, label: snippets[k].label }))} />
        </div>
        <CodeBlock text={snippets[snip].text} />
      </Card>
    </div>
  );
}

function Mapping({ settings, rt, reload }: { settings: Settings; rt: RuntimeView; reload: () => void }) {
  const { t } = useI18n();
  const toast = useToast();
  const act = useAction();
  const all = settings.model_routes ?? [];
  const isFamily = (r: ModelRoute) => FAMILIES.some((f) => f.pattern === r.pattern);
  const [custom, setCustom] = useState<ModelRoute[]>(all.filter((r) => !isFamily(r)));
  const [family, setFamily] = useState<Record<string, string>>(
    Object.fromEntries(FAMILIES.map((f) => [f.key, all.find((r) => r.pattern === f.pattern)?.target ?? ""])),
  );
  const [bg, setBg] = useState(rt.effective.routing.background_model);
  const [bgMax, setBgMax] = useState(String(rt.effective.routing.background_max_tokens));
  useEffect(() => {
    const routes = settings.model_routes ?? [];
    setCustom(routes.filter((r) => !isFamily(r)));
    setFamily(Object.fromEntries(FAMILIES.map((f) => [f.key, routes.find((r) => r.pattern === f.pattern)?.target ?? ""])));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings]);
  useEffect(() => {
    setBg(rt.effective.routing.background_model);
    setBgMax(String(rt.effective.routing.background_max_tokens));
  }, [rt]);
  const models = settings.models.map((m) => m.id);

  const save = async () => {
    const routes = [
      ...custom.filter((r) => r.pattern.trim() && r.target.trim()),
      ...FAMILIES.filter((f) => family[f.key]).map((f) => ({ pattern: f.pattern, target: family[f.key] })),
    ];
    const r = await act.run(async () => {
      await post("/settings/model-routes", { routes });
      await post("/runtime", { routing: { background_model: bg, background_max_tokens: Number(bgMax) || 512 } });
    });
    if (r !== undefined) toast("success", t("common.saved"));
    reload();
  };

  return (
    <Card
      title={t("proxy.mapping")}
      icon={<Route className="h-5 w-5 text-violet-500" />}
      actions={
        <Button tone="primary" icon={<Save className="h-4 w-4" />} disabled={act.busy} onClick={save}>
          {t("common.save")}
        </Button>
      }
    >
      <ErrorBox error={act.error} />
      <div className="grid gap-5 lg:grid-cols-2">
        <div>
          <h3 className="mb-1 text-sm font-semibold">{t("proxy.family")}</h3>
          <p className="mb-2 text-xs text-slate-500">{t("proxy.family_hint")}</p>
          <div className="divide-y divide-slate-100 dark:divide-slate-800">
            {FAMILIES.map((f) => (
              <SettingRow key={f.key} title={<span className="capitalize">{f.key}</span>} hint={<code>{f.pattern}</code>}>
                <ModelSelect models={models} value={family[f.key]} none={t("proxy.no_mapping")} onChange={(v) => setFamily({ ...family, [f.key]: v })} />
              </SettingRow>
            ))}
            <SettingRow title={t("proxy.background")} hint={t("proxy.background_hint")}>
              <ModelSelect models={models} value={bg} none={t("proxy.off")} onChange={setBg} />
              <input className={`${inputBase} w-20 font-mono`} inputMode="numeric" title="max_tokens ≤" value={bgMax} onChange={(e) => setBgMax(e.target.value.replace(/\D/g, ""))} />
            </SettingRow>
          </div>
        </div>
        <div>
          <div className="mb-1 flex flex-wrap items-center justify-between gap-2">
            <h3 className="text-sm font-semibold">{t("proxy.custom_routes")}</h3>
            <div className="flex gap-2">
              <Button small icon={<Wand2 className="h-3.5 w-3.5" />} onClick={() => setCustom([...custom, ...OPENAI_PRESETS.filter((p) => !custom.some((r) => r.pattern === p.pattern))])}>
                {t("settings.add_presets")}
              </Button>
              <Button small icon={<Plus className="h-3.5 w-3.5" />} onClick={() => setCustom([...custom, { pattern: "", target: "" }])}>
                {t("settings.add_route")}
              </Button>
            </div>
          </div>
          <p className="mb-2 text-xs text-slate-500">{t("settings.routes_hint")}</p>
          <datalist id="ccm-models">
            {models.map((m) => (
              <option key={m} value={m} />
            ))}
          </datalist>
          <div className="space-y-2">
            {custom.map((r, i) => (
              <div key={i} className="flex items-center gap-2">
                <input
                  className={`${inputClass} min-w-0 font-mono`}
                  placeholder="gpt-4o / re:^gpt-"
                  value={r.pattern}
                  onChange={(e) => setCustom(custom.map((x, j) => (j === i ? { ...x, pattern: e.target.value } : x)))}
                />
                <ArrowRight className="h-4 w-4 shrink-0 text-slate-400" />
                <input
                  className={`${inputClass} min-w-0 font-mono`}
                  placeholder="claude-sonnet-4-5"
                  list="ccm-models"
                  value={r.target}
                  onChange={(e) => setCustom(custom.map((x, j) => (j === i ? { ...x, target: e.target.value } : x)))}
                />
                <IconButton danger title={t("common.delete")} onClick={() => setCustom(custom.filter((_, j) => j !== i))}>
                  <Trash2 className="h-4 w-4" />
                </IconButton>
              </div>
            ))}
            {custom.length === 0 && <p className="py-4 text-center text-sm text-slate-400">{t("settings.no_routes")}</p>}
          </div>
        </div>
      </div>
    </Card>
  );
}

function ModelSelect({ models, value, onChange, none }: { models: string[]; value: string; onChange: (v: string) => void; none: string }) {
  return (
    <select className={`${inputBase} w-full min-w-0 sm:w-56`} value={value} onChange={(e) => onChange(e.target.value)}>
      <option value="">{none}</option>
      {Array.from(new Set([...models, value].filter(Boolean))).map((m) => (
        <option key={m} value={m}>
          {m}
        </option>
      ))}
    </select>
  );
}

function ModelTests({ settings }: { settings: Settings }) {
  const { t } = useI18n();
  const [results, setResults] = useState<Record<string, ModelTest | "running">>({});
  const models = settings.models.filter((m) => !settings.disabled_models.includes(m.id));
  const test = async (id: string) => {
    setResults((r) => ({ ...r, [id]: "running" }));
    try {
      const res = await post<ModelTest>("/proxy/test", { model: id });
      setResults((r) => ({ ...r, [id]: res }));
    } catch (e) {
      setResults((r) => ({ ...r, [id]: { ok: false, model: id, mapped_model: id, status: 0, latency_ms: 0, error: String(e), failover: 0 } }));
    }
  };
  return (
    <Card
      title={t("connect.models", { n: models.length })}
      icon={<FlaskConical className="h-5 w-5 text-emerald-500" />}
      actions={
        <Button icon={<FlaskConical className="h-4 w-4" />} disabled={models.length === 0} onClick={() => models.forEach((m) => void test(m.id))}>
          {t("proxy.test_all")}
        </Button>
      }
    >
      <p className="mb-3 text-xs text-slate-500">{t("proxy.test_hint")}</p>
      <div className="grid gap-2 md:grid-cols-2 xl:grid-cols-3">
        {models.map((m) => {
          const r = results[m.id];
          return (
            <div key={m.id} className="flex items-center gap-3 rounded-xl border border-slate-200 p-3 dark:border-slate-800">
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="truncate text-sm font-medium">{m.display_name || m.id}</span>
                  <CopyIcon value={m.id} />
                </div>
                <code className="block truncate text-xs text-slate-500">{m.id}</code>
                {r && r !== "running" && (
                  <div className={`mt-1 truncate text-xs ${r.ok ? "text-emerald-600" : "text-red-500"}`} title={r.error || r.text || ""}>
                    {r.ok
                      ? t("proxy.test_ok", { ms: r.latency_ms, a: r.account_id ?? "?" }) + (r.mapped_model !== m.id ? ` → ${r.mapped_model}` : "")
                      : `${r.status || ""} ${r.error ?? ""}`}
                  </div>
                )}
              </div>
              <Button small disabled={r === "running"} onClick={() => test(m.id)} icon={r === "running" ? <RefreshCw className="h-3.5 w-3.5 animate-spin" /> : undefined}>
                {t("proxy.test")}
              </Button>
            </div>
          );
        })}
        {models.length === 0 && <p className="text-sm text-slate-400">{t("usage.no_data")}</p>}
      </div>
    </Card>
  );
}

function ClaudeCodeSync({ port }: { port: string }) {
  const { t } = useI18n();
  const toast = useToast();
  const status = useLoad(() => get<ClaudeCodeStatus>("/claude-code"));
  const [url, setUrl] = useState(`http://127.0.0.1:${port}`);
  useEffect(() => setUrl(`http://127.0.0.1:${port}`), [port]);
  const act = useAction();
  const s = status.data;
  const synced = !!s?.base_url && s.base_url === url.replace(/\/$/, "");
  return (
    <Card title={t("proxy.cc_title")} icon={<Terminal className="h-5 w-5 text-orange-500" />}>
      <ErrorBox error={status.error ?? act.error} />
      <p className="mb-3 text-xs text-slate-500">{isTauri ? t("proxy.cc_hint_desktop") : t("proxy.cc_hint_server")}</p>
      {s && (
        <div className="mb-3 flex flex-wrap items-center gap-2 text-sm">
          <code className="rounded-lg bg-slate-100 px-2 py-1 text-xs dark:bg-slate-800">{s.path}</code>
          {synced ? (
            <Badge tone="green">{t("proxy.cc_synced")}</Badge>
          ) : s.base_url ? (
            <Badge tone="yellow">{t("proxy.cc_other", { u: s.base_url })}</Badge>
          ) : (
            <Badge>{s.exists ? t("proxy.cc_not_set") : t("proxy.cc_no_file")}</Badge>
          )}
          {s.backup && <Badge tone="blue">{t("proxy.cc_backup")}</Badge>}
        </div>
      )}
      <div className="flex flex-wrap items-end gap-2">
        <div className="min-w-56 flex-1">
          <Field label={t("connect.url")}>
            <input className={`${inputClass} font-mono`} value={url} onChange={(e) => setUrl(e.target.value)} />
          </Field>
        </div>
        <Button
          tone="primary"
          disabled={act.busy || !url.trim()}
          onClick={async () => {
            const r = await act.run(() => post<ClaudeCodeStatus>("/claude-code/sync", { base_url: url.trim() }));
            if (r) toast("success", t("proxy.cc_done"));
            await status.reload();
          }}
        >
          {t("proxy.cc_sync")}
        </Button>
        <Button
          icon={<Undo2 className="h-4 w-4" />}
          disabled={act.busy || !s?.backup}
          onClick={async () => {
            if (!confirm(t("proxy.cc_restore_confirm"))) return;
            const r = await act.run(() => post<ClaudeCodeStatus>("/claude-code/restore"));
            if (r) toast("success", t("proxy.cc_restored"));
            await status.reload();
          }}
        >
          {t("proxy.cc_restore")}
        </Button>
      </div>
    </Card>
  );
}

function useCopy() {
  const [copied, setCopied] = useState(false);
  const copy = async (text: string) => {
    await navigator.clipboard.writeText(text).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return { copied, copy };
}

function CodeBlock({ text }: { text: string }) {
  const { t } = useI18n();
  const { copied, copy } = useCopy();
  return (
    <div className="relative">
      <pre className="min-h-40 overflow-x-auto rounded-xl bg-slate-900 p-4 pr-24 text-xs leading-relaxed text-slate-100 dark:bg-slate-950">{text}</pre>
      <Button small className="absolute right-3 top-3" icon={copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />} onClick={() => copy(text)}>
        {copied ? t("common.copied") : t("common.copy")}
      </Button>
    </div>
  );
}

function CopyIcon({ value }: { value: string }) {
  const { t } = useI18n();
  const { copied, copy } = useCopy();
  return (
    <button onClick={() => copy(value)} title={t("common.copy")} className="text-slate-400 hover:text-slate-600">
      {copied ? <Check className="h-3.5 w-3.5 text-emerald-500" /> : <Copy className="h-3.5 w-3.5" />}
    </button>
  );
}
