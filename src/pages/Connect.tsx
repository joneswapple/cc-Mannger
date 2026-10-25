import { useState } from "react";
import { get, isTauri } from "../api";
import type { Status } from "../types";
import { Button, Card, Field, inputClass, useLoad } from "../ui";

/// Generates client setup snippets. Nothing here is sent anywhere: the key
/// is only pasted in to fill the templates.
export default function Connect() {
  const status = useLoad(() => get<Status>("/status"));
  const port = status.data?.listen.split(":").pop() ?? "8787";
  const defaultUrl = isTauri ? `http://<本机局域网 IP>:${port}` : location.origin;
  const [url, setUrl] = useState(defaultUrl);
  const [key, setKey] = useState("sk-cp-...");

  const shell = `export ANTHROPIC_BASE_URL=${url}\nexport ANTHROPIC_API_KEY=${key}\nclaude`;
  const powershell = `$env:ANTHROPIC_BASE_URL = "${url}"\n$env:ANTHROPIC_API_KEY = "${key}"\nclaude`;
  const settings = JSON.stringify({ env: { ANTHROPIC_BASE_URL: url, ANTHROPIC_API_KEY: key } }, null, 2);
  const openai = `from openai import OpenAI\n\nclient = OpenAI(base_url="${url}/v1", api_key="${key}")\nresp = client.chat.completions.create(\n    model="claude-sonnet-4-5",\n    messages=[{"role": "user", "content": "hi"}],\n)\nprint(resp.choices[0].message.content)`;
  const anthropic = `import anthropic\n\nclient = anthropic.Anthropic(base_url="${url}", api_key="${key}")\nmsg = client.messages.create(\n    model="claude-sonnet-4-5", max_tokens=1024,\n    messages=[{"role": "user", "content": "hi"}],\n)\nprint(msg.content[0].text)`;

  return (
    <div className="space-y-4">
      <Card title="接入参数">
        <div className="grid gap-2 sm:grid-cols-2">
          <Field label="服务地址" hint="终端能访问到的地址">
            <input className={`${inputClass} font-mono`} value={url} onChange={(e) => setUrl(e.target.value)} />
          </Field>
          <Field label="API Key" hint="在「终端」页签发；只用于生成下面的片段">
            <input className={`${inputClass} font-mono`} value={key} onChange={(e) => setKey(e.target.value)} />
          </Field>
        </div>
      </Card>
      <Snippet title="Claude Code（macOS / Linux）" text={shell} />
      <Snippet title="Claude Code（Windows PowerShell）" text={powershell} />
      <Snippet title="Claude Code ~/.claude/settings.json" text={settings} />
      <Snippet title="Anthropic Python SDK" text={anthropic} />
      <Snippet title="OpenAI 兼容（Python SDK）" text={openai} />
    </div>
  );
}

function Snippet({ title, text }: { title: string; text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Card
      title={title}
      actions={
        <Button
          small
          onClick={async () => {
            await navigator.clipboard.writeText(text).catch(() => {});
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          }}
        >
          {copied ? "已复制" : "复制"}
        </Button>
      }
    >
      <pre className="overflow-x-auto rounded bg-slate-100 p-3 text-xs dark:bg-slate-950">{text}</pre>
    </Card>
  );
}
