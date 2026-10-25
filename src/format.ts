export function fmtTime(ts: number | null | undefined): string {
  if (!ts) return "—";
  return new Date(ts * 1000).toLocaleString();
}

export function fmtRelative(ts: number | null | undefined, now = Date.now() / 1000): string {
  if (!ts) return "—";
  const d = Math.round(ts - now);
  const abs = Math.abs(d);
  const unit =
    abs < 60 ? `${abs} 秒` : abs < 3600 ? `${Math.round(abs / 60)} 分钟` : abs < 86400 ? `${Math.round(abs / 3600)} 小时` : `${Math.round(abs / 86400)} 天`;
  return d >= 0 ? `${unit}后` : `${unit}前`;
}

export function fmtDuration(secs: number): string {
  if (secs <= 0) return "0 秒";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  if (h) return `${h} 小时 ${m} 分`;
  if (m) return `${m} 分 ${s} 秒`;
  return `${s} 秒`;
}

export function fmtNum(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 10_000) return `${(n / 1_000).toFixed(1)}k`;
  return n.toLocaleString();
}

export function fmtPct(v: number | null | undefined): string {
  if (v === null || v === undefined) return "—";
  return `${Math.round(v * 100)}%`;
}
