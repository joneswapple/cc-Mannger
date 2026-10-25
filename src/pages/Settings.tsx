import { useEffect, useState } from "react";
import { get, post } from "../api";
import { fmtTime } from "../format";
import type { ModelRoute, Settings as SettingsData } from "../types";
import { Button, Card, ErrorBox, inputClass, useAction, useLoad } from "../ui";

export default function Settings() {
  const { data, error, reload } = useLoad(() => get<SettingsData>("/settings"));
  const [disabled, setDisabled] = useState<string[]>([]);
  const [routes, setRoutes] = useState<ModelRoute[]>([]);
  const act = useAction();

  useEffect(() => {
    if (data) {
      setDisabled(data.disabled_models);
      setRoutes(data.model_routes ?? []);
    }
  }, [data]);

  const models = Array.from(new Set([...(data?.models.map((m) => m.id) ?? []), ...disabled]));

  return (
    <div className="space-y-4">
      <ErrorBox error={error ?? act.error} />
      <Card
        title="模型"
        actions={
          <>
            <Button
              disabled={act.busy}
              onClick={async () => {
                await act.run(() => post("/settings/refresh-models"));
                await reload();
              }}
            >
              刷新模型目录
            </Button>
            <Button
              tone="primary"
              disabled={act.busy}
              onClick={async () => {
                await act.run(() => post("/settings/disabled-models", { models: disabled }));
                await reload();
              }}
            >
              保存禁用列表
            </Button>
          </>
        }
      >
        {data && (
          <p className="mb-2 text-xs text-slate-500">
            目录更新于 {fmtTime(data.catalog.last_fetched)}
            {data.catalog.last_error && <span className="text-red-600"> · 上次刷新失败：{data.catalog.last_error}</span>}
          </p>
        )}
        <p className="mb-2 text-xs text-slate-500">勾选的模型对所有终端禁用。</p>
        <div className="grid gap-1 sm:grid-cols-2">
          {models.map((m) => (
            <label key={m} className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={disabled.includes(m)}
                onChange={(e) => setDisabled(e.target.checked ? [...disabled, m] : disabled.filter((x) => x !== m))}
              />
              <code className="text-xs">{m}</code>
              {!data?.models.some((x) => x.id === m) && <span className="text-xs text-amber-600">（已不在目录中）</span>}
            </label>
          ))}
        </div>
      </Card>

      <Card
        title="模型路由"
        actions={
          <>
            <Button onClick={() => setRoutes([...routes, { pattern: "", target: "" }])}>添加规则</Button>
            <Button
              tone="primary"
              disabled={act.busy}
              onClick={async () => {
                await act.run(() => post("/settings/model-routes", { routes: routes.filter((r) => r.pattern.trim()) }));
                await reload();
              }}
            >
              保存
            </Button>
          </>
        }
      >
        <p className="mb-2 text-xs text-slate-500">
          按顺序匹配请求里的模型名，命中第一条就改写为目标模型。精确名称直接写；正则以 <code>re:</code> 开头，例如{" "}
          <code>re:^gpt-4</code> → <code>claude-sonnet-4-5</code>。OpenAI 兼容接口同样生效。
        </p>
        <div className="space-y-2">
          {routes.map((r, i) => (
            <div key={i} className="flex gap-2">
              <input
                className={`${inputClass} font-mono`}
                placeholder="gpt-4o 或 re:^gpt-"
                value={r.pattern}
                onChange={(e) => setRoutes(routes.map((x, j) => (j === i ? { ...x, pattern: e.target.value } : x)))}
              />
              <span className="self-center">→</span>
              <input
                className={`${inputClass} font-mono`}
                placeholder="claude-sonnet-4-5"
                value={r.target}
                onChange={(e) => setRoutes(routes.map((x, j) => (j === i ? { ...x, target: e.target.value } : x)))}
              />
              <Button small tone="danger" onClick={() => setRoutes(routes.filter((_, j) => j !== i))}>
                删除
              </Button>
            </div>
          ))}
          {routes.length === 0 && <p className="text-xs text-slate-500">没有规则，模型名原样转发。</p>}
        </div>
      </Card>

      {data && (
        <Card title="上游参数（只读，在 config.json 中修改）">
          <dl className="grid gap-x-4 gap-y-1 text-sm sm:grid-cols-[12rem_1fr]">
            {(
              [
                ["当前凭据", data.upstream.source],
                ["上游地址", data.upstream.base_url],
                ["anthropic-beta", data.upstream.anthropic_beta],
                ["OAuth token URL", data.upstream.oauth_token_url],
                ["OAuth client id", data.upstream.oauth_client_id],
                ["User-Agent", data.upstream.user_agent],
                ["注入 Claude Code system", data.upstream.inject_claude_code_system ? "是" : "否"],
              ] as const
            ).map(([k, v]) => (
              <div key={k} className="contents">
                <dt className="text-slate-500">{k}</dt>
                <dd className="break-all font-mono text-xs">{v}</dd>
              </div>
            ))}
          </dl>
          <h3 className="mb-1 mt-4 text-xs font-medium text-slate-500">effort 档位</h3>
          <table className="text-xs">
            <tbody>
              {Object.entries(data.upstream.effort_levels).map(([name, l]) => (
                <tr key={name}>
                  <td className="pr-4 font-mono">{name}</td>
                  <td className="pr-4">max_tokens {l.max_tokens}</td>
                  <td>{l.thinking_budget ? `thinking ${l.thinking_budget}` : "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
      )}
    </div>
  );
}
