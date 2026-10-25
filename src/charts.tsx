// Small HTML charts for the stats pages. Colors come from the --series-* and
// --heat-* tokens in index.css (validated categorical order, fixed per
// entity; sequential one-hue ramp), so light and dark each use their own
// steps. Every chart has a hover tooltip; identity is never color alone
// (legend + values in the tooltip and the tables beside the charts).
import { useState, type ReactNode } from "react";

export const SERIES_VARS = ["var(--series-1)", "var(--series-2)", "var(--series-3)", "var(--series-4)", "var(--series-5)"];
export const OTHER_VAR = "var(--series-other)";

export interface Series {
  key: string;
  label: string;
  color: string;
  values: number[];
}

/// Keeps the first `max` keys (callers pass them biggest first, by a measure
/// that doesn't change with the displayed metric) as their own series and
/// folds the rest into "Other", so a key keeps its color across metrics.
export function topSeries(keys: string[], valuesFor: (k: string) => number[], label: (k: string) => string, otherLabel: string, max = SERIES_VARS.length): Series[] {
  const totals = keys.map((k) => ({ k, v: valuesFor(k) }));
  const top = totals.slice(0, max);
  const rest = totals.slice(max);
  const out: Series[] = top.map((x, i) => ({ key: x.k, label: label(x.k), color: SERIES_VARS[i], values: x.v }));
  if (rest.length) {
    const n = rest[0].v.length;
    out.push({ key: "__other", label: otherLabel, color: OTHER_VAR, values: Array.from({ length: n }, (_, i) => rest.reduce((s, x) => s + x.v[i], 0)) });
  }
  return out;
}

function niceMax(v: number): number {
  if (v <= 0) return 1;
  const p = Math.pow(10, Math.floor(Math.log10(v)));
  for (const m of [1, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}

export function Legend({ series }: { series: Series[] }) {
  if (series.length < 2) return null;
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-slate-600 dark:text-slate-300">
      {series.map((s) => (
        <span key={s.key} className="flex items-center gap-1.5">
          <span className="h-2.5 w-2.5 rounded-sm" style={{ background: s.color }} />
          {s.label}
        </span>
      ))}
    </div>
  );
}

/// Stacked columns over time. `labels` names each bucket (x axis).
export function StackedBars({
  labels,
  series,
  format,
  height = 200,
  empty,
  tickEvery,
}: {
  labels: string[];
  series: Series[];
  format: (v: number) => string;
  height?: number;
  empty: ReactNode;
  tickEvery?: number;
}) {
  const [hover, setHover] = useState<number | null>(null);
  const totals = labels.map((_, i) => series.reduce((s, x) => s + (x.values[i] ?? 0), 0));
  const max = niceMax(Math.max(0, ...totals));
  if (totals.every((t) => t === 0)) return <div className="flex items-center justify-center text-sm text-slate-400" style={{ height }}>{empty}</div>;
  const every = tickEvery ?? Math.max(1, Math.ceil(labels.length / 12));
  const ticks = [0, 0.5, 1];
  return (
    <div className="flex gap-2">
      <div className="relative w-12 shrink-0 text-right text-[10px] tabular-nums text-slate-400" style={{ height }}>
        {ticks.map((f) => (
          <span key={f} className="absolute right-0 -translate-y-1/2" style={{ top: `${(1 - f) * 100}%` }}>
            {format(max * f)}
          </span>
        ))}
      </div>
      <div className="min-w-0 flex-1">
        <div className="relative" style={{ height }} onMouseLeave={() => setHover(null)}>
          {ticks.map((f) => (
            <div key={f} className="absolute inset-x-0 border-t" style={{ top: `${(1 - f) * 100}%`, borderColor: f === 0 ? "var(--chart-axis)" : "var(--chart-grid)" }} />
          ))}
          <div className="absolute inset-0 flex items-end gap-[2px]">
            {labels.map((l, i) => (
              <div key={l} className="relative flex h-full min-w-0 flex-1 flex-col justify-end" onMouseEnter={() => setHover(i)}>
                <div className={`flex w-full flex-col-reverse gap-[2px] transition-opacity ${hover !== null && hover !== i ? "opacity-60" : ""}`} style={{ height: `${(totals[i] / max) * 100}%` }}>
                  {series.map((s, si) => {
                    const v = s.values[i] ?? 0;
                    if (!v) return null;
                    const isTop = series.slice(si + 1).every((x) => !(x.values[i] ?? 0));
                    return <div key={s.key} className={isTop ? "rounded-t" : ""} style={{ flex: v, minHeight: 2, background: s.color }} />;
                  })}
                </div>
              </div>
            ))}
          </div>
          {hover !== null && (
            <Tooltip x={(hover + 0.5) / labels.length}>
              <div className="mb-1 font-medium">{labels[hover]}</div>
              {series
                .filter((s) => s.values[hover])
                .map((s) => (
                  <div key={s.key} className="flex items-center gap-2">
                    <span className="h-2 w-2 rounded-sm" style={{ background: s.color }} />
                    <span className="truncate">{s.label}</span>
                    <span className="ml-auto pl-3 tabular-nums">{format(s.values[hover])}</span>
                  </div>
                ))}
              {series.length > 1 && (
                <div className="mt-1 flex border-t border-slate-200 pt-1 dark:border-slate-700">
                  <span>Σ</span>
                  <span className="ml-auto tabular-nums">{format(totals[hover])}</span>
                </div>
              )}
            </Tooltip>
          )}
        </div>
        <div className="mt-1 flex gap-[2px]">
          {labels.map((l, i) => (
            <div key={l} className="min-w-0 flex-1 overflow-visible whitespace-nowrap text-center text-[10px] text-slate-400">
              {i % every === 0 ? l : ""}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function Tooltip({ x, children }: { x: number; children: ReactNode }) {
  const left = x > 0.6;
  return (
    <div
      className="pointer-events-none absolute top-2 z-10 min-w-40 max-w-64 rounded-xl border border-slate-200 bg-white/95 px-3 py-2 text-xs text-slate-700 shadow-lg backdrop-blur dark:border-slate-700 dark:bg-slate-900/95 dark:text-slate-200"
      style={left ? { right: `${(1 - x) * 100 + 2}%` } : { left: `${x * 100 + 2}%` }}
    >
      {children}
    </div>
  );
}

export interface HeatCell {
  weekday: number;
  hour: number;
  requests: number;
  tokens: number;
}

/// Weekday × hour grid (Monday first), shaded by request count.
export function Heatmap({ cells, weekdays, format, legend }: { cells: HeatCell[]; weekdays: string[]; format: (c: HeatCell) => string; legend: [string, string] }) {
  const [hover, setHover] = useState<HeatCell | null>(null);
  const grid = new Map(cells.map((c) => [`${c.weekday}-${c.hour}`, c]));
  const max = Math.max(1, ...cells.map((c) => c.requests));
  const step = (v: number) => (v <= 0 ? 0 : Math.min(7, 1 + Math.floor((v / max) * 6.999)));
  return (
    <div className="relative">
      <div className="overflow-x-auto">
        <div className="min-w-[36rem]">
          {weekdays.map((d, wd) => (
            <div key={d} className="mb-[3px] flex items-center gap-[3px]">
              <span className="w-8 shrink-0 text-[10px] text-slate-400">{d}</span>
              {Array.from({ length: 24 }, (_, h) => {
                const c = grid.get(`${wd}-${h}`) ?? { weekday: wd, hour: h, requests: 0, tokens: 0 };
                return (
                  <div
                    key={h}
                    className="h-5 flex-1 rounded-[4px] ring-slate-900/40 hover:ring-2 dark:ring-white/50"
                    style={{ background: `var(--heat-${step(c.requests)})` }}
                    onMouseEnter={() => setHover(c)}
                    onMouseLeave={() => setHover(null)}
                  />
                );
              })}
            </div>
          ))}
          <div className="flex gap-[3px] pl-[35px]">
            {Array.from({ length: 24 }, (_, h) => (
              <span key={h} className="flex-1 text-center text-[10px] text-slate-400">
                {h % 3 === 0 ? h : ""}
              </span>
            ))}
          </div>
        </div>
      </div>
      <div className="mt-2 flex flex-wrap items-center justify-between gap-2 text-xs text-slate-500">
        <span className="min-h-4 font-mono">{hover ? `${weekdays[hover.weekday]} ${String(hover.hour).padStart(2, "0")}:00 · ${format(hover)}` : ""}</span>
        <span className="flex items-center gap-1">
          {legend[0]}
          {[1, 2, 3, 4, 5, 6, 7].map((i) => (
            <span key={i} className="h-3 w-3 rounded-sm" style={{ background: `var(--heat-${i})` }} />
          ))}
          {legend[1]}
        </span>
      </div>
    </div>
  );
}
