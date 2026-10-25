import { useState } from "react";
import { get } from "../api";
import { fmtNum, fmtTime } from "../format";
import type { Stats, UsagePage } from "../types";
import { Badge, Button, Card, ErrorBox, inputClass, useLoad } from "../ui";

export default function Usage() {
  const [days, setDays] = useState(7);
  const stats = useLoad(() => get<Stats>(`/stats?days=${days}`), [days]);
  const [filter, setFilter] = useState({ q: "", model: "", status: "", sort: "time_desc" });
  const [applied, setApplied] = useState(filter);
  const [page, setPage] = useState(1);
  const usage = useLoad(() => {
    const qs = new URLSearchParams({ ...applied, page: String(page), page_size: "50" });
    return get<UsagePage>(`/usage?${qs}`);
  }, [applied, page]);

  const maxDay = Math.max(1, ...(stats.data?.by_day.map((d) => d.input_tokens + d.output_tokens) ?? [1]));
  const pages = usage.data ? Math.max(1, Math.ceil(usage.data.total / usage.data.page_size)) : 1;

  return (
    <div className="space-y-4">
      <Card
        title="汇总"
        actions={
          <select className={inputClass} value={days} onChange={(e) => setDays(Number(e.target.value))}>
            {[1, 7, 30, 90].map((d) => (
              <option key={d} value={d}>
                最近 {d} 天
              </option>
            ))}
          </select>
        }
      >
        <ErrorBox error={stats.error} />
        {stats.data && (
          <div className="space-y-4">
            <div>
              <h3 className="mb-2 text-xs font-medium text-slate-500">每日 token</h3>
              <div className="flex h-24 items-end gap-1">
                {stats.data.by_day.map((d) => {
                  const total = d.input_tokens + d.output_tokens;
                  return (
                    <div key={d.date} className="flex flex-1 flex-col items-center" title={`${d.date}：${d.requests} 次，${total.toLocaleString()} tokens`}>
                      <div className="w-full rounded-t bg-orange-500" style={{ height: `${(total / maxDay) * 100}%`, minHeight: total ? 2 : 0 }} />
                    </div>
                  );
                })}
                {stats.data.by_day.length === 0 && <span className="text-xs text-slate-500">暂无数据</span>}
              </div>
            </div>
            <div className="grid gap-4 md:grid-cols-3">
              <Rollup title="按模型" rows={stats.data.by_model.map((r) => ({ name: r.model, ...r }))} />
              <Rollup title="按终端" rows={stats.data.by_user.map((r) => ({ name: r.label, ...r }))} />
              <Rollup title="按上游账号" rows={stats.data.by_account.map((r) => ({ name: `#${r.account_id} ${r.label}`, ...r }))} />
            </div>
          </div>
        )}
      </Card>
      <Card title={`请求记录（${usage.data?.total ?? 0}）`}>
        <form
          className="mb-3 flex flex-wrap gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            setPage(1);
            setApplied(filter);
          }}
        >
          <input className={`${inputClass} w-48`} placeholder="搜索模型 / 用户 / 错误" value={filter.q} onChange={(e) => setFilter({ ...filter, q: e.target.value })} />
          <input className={`${inputClass} w-40`} placeholder="模型" value={filter.model} onChange={(e) => setFilter({ ...filter, model: e.target.value })} />
          <select className={`${inputClass} w-32`} value={filter.status} onChange={(e) => setFilter({ ...filter, status: e.target.value })}>
            <option value="">全部状态</option>
            <option value="ok">成功</option>
            <option value="error">失败</option>
            <option value="429">429</option>
            <option value="5xx">5xx</option>
          </select>
          <select className={`${inputClass} w-36`} value={filter.sort} onChange={(e) => setFilter({ ...filter, sort: e.target.value })}>
            <option value="time_desc">最新优先</option>
            <option value="tokens_desc">token 最多</option>
            <option value="latency_desc">耗时最长</option>
          </select>
          <Button type="submit">查询</Button>
        </form>
        <ErrorBox error={usage.error} />
        <div className="overflow-x-auto">
          <table className="w-full text-left text-xs">
            <thead className="text-slate-500">
              <tr>
                <th className="py-1">时间</th>
                <th>用户</th>
                <th>账号</th>
                <th>模型</th>
                <th className="text-right">输入</th>
                <th className="text-right">输出</th>
                <th className="text-right">缓存读</th>
                <th className="text-right">耗时</th>
                <th>状态</th>
              </tr>
            </thead>
            <tbody>
              {usage.data?.events.map((e) => (
                <tr key={e.id} className="border-t border-slate-100 align-top dark:border-slate-800" title={e.error}>
                  <td className="whitespace-nowrap py-1">{fmtTime(e.ts)}</td>
                  <td>{e.user_label}</td>
                  <td>{e.account_id ? `#${e.account_id}` : "—"}</td>
                  <td className="font-mono">{e.model}</td>
                  <td className="text-right">{fmtNum(e.input_tokens)}</td>
                  <td className="text-right">{fmtNum(e.output_tokens)}</td>
                  <td className="text-right">{fmtNum(e.cache_read_tokens)}</td>
                  <td className="text-right">{(e.latency_ms / 1000).toFixed(1)}s</td>
                  <td>
                    <Badge tone={e.status < 400 ? "green" : e.status === 429 ? "yellow" : "red"}>{e.status}</Badge>
                    {e.error && <div className="max-w-xs truncate text-slate-500">{e.error}</div>}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <div className="mt-3 flex items-center gap-2 text-xs">
          <Button small disabled={page <= 1} onClick={() => setPage(page - 1)}>
            上一页
          </Button>
          <span>
            {page} / {pages}
          </span>
          <Button small disabled={page >= pages} onClick={() => setPage(page + 1)}>
            下一页
          </Button>
        </div>
      </Card>
    </div>
  );
}

function Rollup({ title, rows }: { title: string; rows: { name: string; requests: number; input_tokens: number; output_tokens: number }[] }) {
  return (
    <div>
      <h3 className="mb-1 text-xs font-medium text-slate-500">{title}</h3>
      <table className="w-full text-xs">
        <tbody>
          {rows.map((r) => (
            <tr key={r.name} className="border-t border-slate-100 dark:border-slate-800">
              <td className="max-w-[10rem] truncate py-1" title={r.name}>
                {r.name}
              </td>
              <td className="text-right">{r.requests} 次</td>
              <td className="text-right">{fmtNum(r.input_tokens + r.output_tokens)}</td>
            </tr>
          ))}
          {rows.length === 0 && (
            <tr>
              <td className="text-slate-500">暂无数据</td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}
