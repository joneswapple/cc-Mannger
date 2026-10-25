import { Check, Cloud, Copy, Download, ExternalLink, Play, RefreshCw, Save, ScrollText, Square, TriangleAlert, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { get, openExternal, post } from "../api";
import { fmtRelative } from "../format";
import { useI18n } from "../i18n";
import { useToast } from "../toast";
import type { TunnelConfig, TunnelView } from "../types";
import { Badge, Button, Card, ErrorBox, Segmented, SettingRow, Switch, inputBase, useAction, useLoad } from "../ui";

/// Public access through a Cloudflare tunnel (API proxy page).
export default function TunnelCard({ onUrl }: { onUrl?: (url: string | null) => void }) {
  const { t } = useI18n();
  const toast = useToast();
  const view = useLoad(() => get<TunnelView>("/tunnel"));
  const act = useAction();
  const [token, setToken] = useState("");
  const [port, setPort] = useState("");
  const [showLogs, setShowLogs] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  const [copied, setCopied] = useState(false);
  const s = view.data?.status;
  const cfg = view.data?.config;

  useEffect(() => {
    if (cfg) setPort(String(cfg.local_port));
  }, [cfg?.local_port]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => onUrl?.(s?.url ?? null), [s?.url]); // eslint-disable-line react-hooks/exhaustive-deps

  // Poll while starting (no URL yet) or installing, and the logs when open.
  const pending = !!s && ((s.running && !s.url) || s.installing);
  useEffect(() => {
    if (!pending && !showLogs) return;
    const id = setInterval(async () => {
      if (pending) void view.reload();
      if (showLogs) setLogs((await get<{ lines: string[] }>("/tunnel/logs").catch(() => ({ lines: [] }))).lines);
    }, 1500);
    return () => clearInterval(id);
  }, [pending, showLogs, view.reload]);
  useEffect(() => {
    if (showLogs) get<{ lines: string[] }>("/tunnel/logs").then((r) => setLogs(r.lines), () => {});
  }, [showLogs]);

  const run = async (fn: () => Promise<unknown>, ok?: string) => {
    const r = await act.run(fn);
    if (r !== undefined && ok) toast("success", ok);
    await view.reload();
  };
  const setCfg = (patch: Partial<TunnelConfig>) => run(() => post("/runtime", { tunnel: patch }), t("common.saved"));

  if (!s || !cfg) {
    return (
      <Card title={t("tunnel.title")} icon={<Cloud className="h-5 w-5 text-orange-500" />}>
        <ErrorBox error={view.error} />
      </Card>
    );
  }
  const locked = s.running; // settings apply on the next start
  return (
    <Card
      title={t("tunnel.title")}
      icon={<Cloud className="h-5 w-5 text-orange-500" />}
      actions={
        <>
          {s.running ? (
            s.url ? (
              <Badge tone="green">● {t("tunnel.online")}</Badge>
            ) : (
              <Badge tone="yellow">{t("tunnel.connecting")}</Badge>
            )
          ) : (
            <Badge>{t("tunnel.off")}</Badge>
          )}
          {!s.installed ? (
            <Button tone="primary" icon={<Download className={`h-4 w-4 ${s.installing ? "animate-bounce" : ""}`} />} disabled={act.busy || s.installing} onClick={() => run(() => post("/tunnel/install"), t("tunnel.installed_ok"))}>
              {s.installing ? t("tunnel.installing") : t("tunnel.install")}
            </Button>
          ) : s.running ? (
            <Button tone="danger" icon={<Square className="h-4 w-4" />} disabled={act.busy} onClick={() => run(() => post("/tunnel/stop"))}>
              {t("proxy.stop")}
            </Button>
          ) : (
            <Button tone="primary" icon={<Play className="h-4 w-4" />} disabled={act.busy || (cfg.mode === "token" && !s.has_token)} onClick={() => run(() => post("/tunnel/start"))}>
              {t("proxy.start")}
            </Button>
          )}
        </>
      }
    >
      <ErrorBox error={act.error ?? view.error ?? s.error} />
      <div className="mb-3 flex gap-2 rounded-xl border border-amber-200 bg-amber-50/70 p-3 text-xs text-amber-800 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-200">
        <TriangleAlert className="h-4 w-4 shrink-0" />
        <span>{t("tunnel.warning")}</span>
      </div>

      {s.url && (
        <div className="mb-3 flex flex-wrap items-center gap-2 rounded-xl border border-emerald-200 bg-emerald-50/60 p-3 dark:border-emerald-900 dark:bg-emerald-950/40">
          <span className="text-xs text-emerald-700 dark:text-emerald-300">{t("tunnel.public_url")}</span>
          <code className="min-w-0 flex-1 truncate font-mono text-sm">{s.url}</code>
          <Button
            small
            icon={copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
            onClick={async () => {
              await navigator.clipboard.writeText(s.url ?? "").catch(() => {});
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            }}
          >
            {copied ? t("common.copied") : t("common.copy")}
          </Button>
          <Button small icon={<ExternalLink className="h-3.5 w-3.5" />} onClick={() => openExternal(`${s.url}/healthz`)}>
            {t("tunnel.open")}
          </Button>
        </div>
      )}

      <div className="divide-y divide-slate-100 dark:divide-slate-800">
        <SettingRow title={t("tunnel.binary")} hint={s.installed ? `${s.binary ?? ""}${s.managed ? ` · ${t("tunnel.managed")}` : ""}` : t("tunnel.not_installed")}>
          {s.version && <Badge>{s.version.replace(/^cloudflared version /, "")}</Badge>}
          {s.installed && s.managed && (
            <Button small icon={<RefreshCw className="h-3.5 w-3.5" />} disabled={act.busy || s.installing || s.running} onClick={() => run(() => post("/tunnel/install"), t("tunnel.installed_ok"))}>
              {t("tunnel.reinstall")}
            </Button>
          )}
        </SettingRow>
        <SettingRow title={t("tunnel.mode")} hint={cfg.mode === "quick" ? t("tunnel.mode_quick_hint") : t("tunnel.mode_token_hint", { port: cfg.local_port })}>
          <Segmented
            value={cfg.mode}
            disabled={locked}
            onChange={(m) => setCfg({ mode: m })}
            options={[
              { key: "quick", label: t("tunnel.quick") },
              { key: "token", label: t("tunnel.token_mode") },
            ]}
          />
        </SettingRow>
        {cfg.mode === "token" && (
          <SettingRow title={t("tunnel.token")} hint={s.has_token ? t("tunnel.token_saved") : t("tunnel.token_hint")}>
            <input className={`${inputBase} w-56 font-mono`} type="password" autoComplete="off" placeholder={s.has_token ? "••••••••" : "eyJhIjoi…"} value={token} onChange={(e) => setToken(e.target.value)} />
            <Button
              icon={<Save className="h-4 w-4" />}
              disabled={!token.trim() || act.busy}
              onClick={async () => {
                await run(() => post("/tunnel/token", { token }), t("common.saved"));
                setToken("");
              }}
            >
              {t("common.save")}
            </Button>
            {s.has_token && (
              <Button tone="danger" icon={<Trash2 className="h-4 w-4" />} disabled={act.busy || locked} onClick={() => run(() => post("/tunnel/token", { token: null }))}>
                {t("tunnel.clear")}
              </Button>
            )}
          </SettingRow>
        )}
        <SettingRow title={t("tunnel.protocol")} hint={t("tunnel.protocol_hint")}>
          <Segmented value={cfg.protocol} disabled={locked} onChange={(p) => setCfg({ protocol: p })} options={(["auto", "http2", "quic"] as const).map((p) => ({ key: p, label: p.toUpperCase() === "AUTO" ? t("tunnel.auto") : p.toUpperCase() }))} />
        </SettingRow>
        <SettingRow title={t("tunnel.local_port")} hint={t("tunnel.local_port_hint")}>
          <input className={`${inputBase} w-24 font-mono`} inputMode="numeric" disabled={locked} value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))} />
          <Button icon={<Save className="h-4 w-4" />} disabled={locked || !port || port === String(cfg.local_port)} onClick={() => setCfg({ local_port: Number(port) })}>
            {t("common.save")}
          </Button>
        </SettingRow>
        <SettingRow title={t("tunnel.expose_admin")} hint={t("tunnel.expose_admin_hint")}>
          <Switch checked={cfg.expose_admin} disabled={locked} onChange={(v) => setCfg({ expose_admin: v })} />
        </SettingRow>
        <SettingRow title={t("tunnel.auto_start")} hint={t("tunnel.auto_start_hint")}>
          <Switch checked={cfg.auto_start} onChange={(v) => setCfg({ auto_start: v })} />
        </SettingRow>
      </div>
      {locked && <p className="mt-2 text-xs text-slate-500">{t("tunnel.locked_hint", { since: s.started_at ? fmtRelative(s.started_at) : "" })}</p>}
      <div className="mt-3">
        <Button small icon={<ScrollText className="h-3.5 w-3.5" />} onClick={() => setShowLogs(!showLogs)}>
          {showLogs ? t("tunnel.hide_logs") : t("tunnel.show_logs")}
        </Button>
        {showLogs && (
          <pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-xl bg-slate-950 p-3 font-mono text-[11px] leading-5 text-slate-200">
            {logs.length ? logs.join("\n") : t("settings.no_logs")}
          </pre>
        )}
      </div>
    </Card>
  );
}
