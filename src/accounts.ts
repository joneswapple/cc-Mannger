// Derived account state shared by the dashboard, accounts page and tray-like
// summaries. Mirrors the scheduler's view (pool.rs).
import type { Account, Quota } from "./types";

export type Window = "5h" | "7d";

/// Remaining fraction of a window (0–1), or null when unknown. A window
/// whose reset time has passed counts as full.
export function remaining(q: Quota | null | undefined, w: Window, now: number): number | null {
  if (!q) return null;
  const used = w === "5h" ? q.five_hour_utilization : q.seven_day_utilization;
  const reset = w === "5h" ? q.five_hour_reset : q.seven_day_reset;
  if (used === null || used === undefined) return null;
  if (reset && reset <= now) return 1;
  return Math.max(0, Math.min(1, 1 - used));
}

export function resetAt(q: Quota | null | undefined, w: Window): number | null {
  if (!q) return null;
  return (w === "5h" ? q.five_hour_reset : q.seven_day_reset) ?? null;
}

export const isHealthy = (a: Account, now: number) => a.enabled && a.status === "active" && a.cooldown_until <= now;

export const isProblem = (a: Account, now: number) => a.enabled && (a.status !== "active" || a.cooldown_until > now);

/// Lowest remaining across known windows; null when nothing is known.
export function minRemaining(a: Account, now: number): number | null {
  const vals = [remaining(a.quota, "5h", now), remaining(a.quota, "7d", now)].filter((v): v is number => v !== null);
  return vals.length ? Math.min(...vals) : null;
}

export const isLow = (a: Account, now: number) => {
  const r = minRemaining(a, now);
  return r !== null && r < 0.2;
};

export const displayName = (a: Account) => a.email || a.label;

/// Saves JSON as a file in the browser / webview.
export function downloadJson(name: string, data: unknown) {
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
