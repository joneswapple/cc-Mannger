import { get } from "../api";
import { fmtNum, fmtRelative } from "../format";
import type { AccountsResponse, Stats, Status } from "../types";
import { Card, ErrorBox, Meter, useLoad } from "../ui";
import { statusBadge } from "./Accounts";

/// The account the scheduler would pick next for an unrestricted request:
/// mirrors pool.rs (enabled, active, not cooling, under the quota soft
/// limit, then priority / id order).
function recommended(data: AccountsResponse) {
  const soft = data.pool.quota_soft_limit ?? 1;
  const healthy = data.accounts.filter((a) => a.enabled && a.status === "active" && a.cooldown_until <= data.now);
  const roomy = healthy.filter((a) => (a.quota?.five_hour_utilization ?? 0) < soft && (a.quota?.seven_day_utilization ?? 0) < soft);
  return (roomy.length ? roomy : healthy)[0];
}

export default function Dashboard() {
  const status = useLoad(() => get<Status>("/status"));
  const accounts = useLoad(() => get<AccountsResponse>("/accounts"));
  const today = useLoad(() => get<Stats>("/stats?days=1"));

  const list = accounts.data?.accounts ?? [];
  const now = accounts.data?.now ?? Date.now() / 1000;
  const counts = {
    ok: list.filter((a) => a.enabled && a.status === "active" && a.cooldown_until <= now).length,
    cooling: list.filter((a) => a.enabled && a.cooldown_until > now).length,
    reauth: list.filter((a) => a.enabled && a.status === "needs_reauth").length,
  };
  const withQuota = list.filter((a) => a.quota);
  const avg = (f: (q: NonNullable<(typeof list)[number]["quota"]>) => number | null) => {
    const vals = withQuota.map((a) => f(a.quota!)).filter((v): v is number => v !== null);
    return vals.length ? vals.reduce((s, v) => s + v, 0) / vals.length : null;
  };
  const day = today.data?.by_day.reduce(
    (s, d) => ({ requests: s.requests + d.requests, tokens: s.tokens + d.input_tokens + d.output_tokens }),
    { requests: 0, tokens: 0 },
  );
  const rec = accounts.data ? recommended(accounts.data) : undefined;

  return (
    <div className="space-y-4">
      <ErrorBox error={status.error ?? accounts.error} />
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <Stat label="可用账号" value={`${counts.ok} / ${list.length}`} />
        <Stat label="冷却 / 需重新登录" value={`${counts.cooling} / ${counts.reauth}`} />
        <Stat label="今日请求" value={fmtNum(day?.requests ?? 0)} />
        <Stat label="今日 token" value={fmtNum(day?.tokens ?? 0)} />
      </div>
      <div className="grid gap-4 lg:grid-cols-2">
        <Card title="额度（所有账号平均）">
          {withQuota.length === 0 ? (
            <p className="text-sm text-slate-500">还没有额度数据：账号处理过请求后会从上游响应头里记录。</p>
          ) : (
            <div className="space-y-3">
              <Meter label="5 小时窗口" value={avg((q) => q.five_hour_utilization)} />
              <Meter label="7 天窗口" value={avg((q) => q.seven_day_utilization)} />
            </div>
          )}
          {rec && (
            <p className="mt-4 text-sm">
              推荐账号：<span className="font-medium">#{rec.id} {rec.label}</span> {statusBadge(rec, now)}
              {rec.quota && <span className="ml-2 text-xs text-slate-500">5h 重置 {fmtRelative(rec.quota.five_hour_reset, now)}</span>}
            </p>
          )}
        </Card>
        <Card title="服务">
          {status.data && (
            <dl className="grid grid-cols-[7rem_1fr] gap-y-1 text-sm">
              <dt className="text-slate-500">版本</dt>
              <dd>{status.data.version}</dd>
              <dt className="text-slate-500">监听地址</dt>
              <dd className="font-mono">{status.data.listen}</dd>
              <dt className="text-slate-500">当前上游</dt>
              <dd className="break-all text-xs">{status.data.upstream}</dd>
              <dt className="text-slate-500">模型目录</dt>
              <dd>
                {status.data.catalog.models} 个
                {status.data.catalog.last_error && <span className="text-xs text-red-600"> · {status.data.catalog.last_error}</span>}
              </dd>
            </dl>
          )}
        </Card>
      </div>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900">
      <div className="text-xs text-slate-500">{label}</div>
      <div className="mt-1 text-xl font-semibold">{value}</div>
    </div>
  );
}
