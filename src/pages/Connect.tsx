import { Check, Copy, Globe, KeyRound, Link2, Radio, Terminal } from "lucide-react";
import { useState } from "react";
import { get, isTauri } from "../api";
import { useI18n } from "../i18n";
import type { Settings, Status } from "../types";
import { Badge, Button, Card, Field, Segmented, inputClass, useLoad } from "../ui";

type Snip = "shell" | "powershell" | "settings" | "anthropic" | "openai" | "curl";

/// Generates client setup snippets. Nothing here is sent anywhere: the key
/// is only pasted in to fill the templates.
export default function Connect() {
  const { t } = useI18n();
  const status = useLoad(() => get<Status>("/status"));
  const settings = useLoad(() => get<Settings>("/settings"));
  const port = status.data?.listen.split(":").pop() ?? "8787";
  const defaultUrl = isTauri ? `http://${t("connect.lan_ip")}:${port}` : location.origin;
  const [url, setUrl] = useState(defaultUrl);
  const [key, setKey] = useState("sk-cp-...");
  const [snip, setSnip] = useState<Snip>("shell");
  const models = (settings.data?.models ?? []).filter((m) => !settings.data?.disabled_models.includes(m.id));
  const model = models.find((m) => m.id.includes("sonnet"))?.id ?? models[0]?.id ?? "claude-sonnet-4-5";

  const snippets: Record<Snip, { label: string; text: string }> = {
    shell: { label: "macOS / Linux", text: `export ANTHROPIC_BASE_URL=${url}\nexport ANTHROPIC_API_KEY=${key}\nclaude` },
    powershell: { label: "PowerShell", text: `$env:ANTHROPIC_BASE_URL = "${url}"\n$env:ANTHROPIC_API_KEY = "${key}"\nclaude` },
    settings: { label: "settings.json", text: `// ~/.claude/settings.json\n${JSON.stringify({ env: { ANTHROPIC_BASE_URL: url, ANTHROPIC_API_KEY: key } }, null, 2)}` },
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
    <div className="space-y-5">
      <h1 className="text-2xl font-bold tracking-tight">{t("nav.connect")}</h1>

      <Card bodyClass="p-5">
        <div className="flex flex-wrap items-center gap-4">
          <div className="flex h-12 w-12 items-center justify-center rounded-2xl bg-emerald-50 dark:bg-emerald-950">
            <Radio className="h-6 w-6 text-emerald-500" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-lg font-semibold">{t("connect.service")}</span>
              {status.data ? <Badge tone="green">● {t("connect.running")}</Badge> : <Badge tone="red">{t("connect.unreachable")}</Badge>}
            </div>
            <div className="mt-0.5 text-sm text-slate-500">
              {status.data && t("connect.service_desc", { listen: status.data.listen, n: status.data.catalog.models, a: status.data.accounts })}
            </div>
          </div>
        </div>
      </Card>

      <div className="grid gap-4 lg:grid-cols-5">
        <Card title={t("connect.params")} icon={<Link2 className="h-5 w-5 text-blue-500" />} className="min-w-0 lg:col-span-2">
          <div className="space-y-3">
            <Field label={t("connect.url")} hint={t("connect.url_hint")}>
              <div className="relative">
                <Globe className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-400" />
                <input className={`${inputClass} pl-9 font-mono`} value={url} onChange={(e) => setUrl(e.target.value)} />
              </div>
            </Field>
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

      <Card title={t("connect.models", { n: models.length })}>
        <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
          {models.map((m) => (
            <CopyRow key={m.id} label={m.display_name} value={m.id} />
          ))}
          {models.length === 0 && <p className="text-sm text-slate-400">{t("usage.no_data")}</p>}
        </div>
      </Card>
    </div>
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

function CopyRow({ label, value }: { label: string; value: string }) {
  const { t } = useI18n();
  const { copied, copy } = useCopy();
  return (
    <button
      onClick={() => copy(value)}
      title={t("common.copy")}
      className="flex items-center gap-3 rounded-xl border border-slate-200 p-3 text-left transition-colors hover:bg-slate-50 dark:border-slate-800 dark:hover:bg-slate-800/40"
    >
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-medium">{label || value}</div>
        <code className="block truncate text-xs text-slate-500">{value}</code>
      </div>
      {copied ? <Check className="h-4 w-4 shrink-0 text-emerald-500" /> : <Copy className="h-4 w-4 shrink-0 text-slate-400" />}
    </button>
  );
}
