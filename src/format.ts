export function fmtTime(ts: number | null | undefined): string {
  if (!ts) return "—";
  return new Date(ts * 1000).toLocaleString();
}

// The UI language for the helpers below; set by LangProvider on render.
let zh = true;
export function setFormatLang(lang: "zh" | "en") {
  zh = lang === "zh";
}

export function fmtRelative(ts: number | null | undefined, now = Date.now() / 1000): string {
  if (!ts) return "—";
  const d = Math.round(ts - now);
  const abs = Math.abs(d);
  const [n, zhUnit, enUnit] =
    abs < 60 ? [abs, "秒", "s"] : abs < 3600 ? [Math.round(abs / 60), "分钟", "min"] : abs < 86400 ? [Math.round(abs / 3600), "小时", "h"] : [Math.round(abs / 86400), "天", "d"];
  if (zh) return d >= 0 ? `${n} ${zhUnit}后` : `${n} ${zhUnit}前`;
  return d >= 0 ? `in ${n}${enUnit}` : `${n}${enUnit} ago`;
}

export function fmtDuration(secs: number): string {
  const h = Math.floor(Math.max(0, secs) / 3600);
  const m = Math.floor((Math.max(0, secs) % 3600) / 60);
  const s = Math.floor(Math.max(0, secs) % 60);
  if (zh) {
    if (h) return `${h} 小时 ${m} 分`;
    if (m) return `${m} 分 ${s} 秒`;
    return `${s} 秒`;
  }
  if (h) return `${h}h ${m}m`;
  if (m) return `${m}m ${s}s`;
  return `${s}s`;
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

/// Compact countdown like the reset timers: "45s", "18m", "1h 18m", "6d 15h".
export function fmtCountdown(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${m}m`;
  if (m) return `${m}m`;
  return `${s}s`;
}

export function fmtDate(ts: number | null | undefined): string {
  if (!ts) return "—";
  return new Date(ts * 1000).toLocaleDateString();
}

export function fmtClock(ts: number | null | undefined): string {
  if (!ts) return "";
  return new Date(ts * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
