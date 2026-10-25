import { ChevronLeft, ChevronRight, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { fmtCountdown } from "./format";
import { useI18n } from "./i18n";

export function Card({
  title,
  icon,
  actions,
  children,
  className = "",
  bodyClass = "p-5",
}: {
  title?: ReactNode;
  icon?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
  bodyClass?: string;
}) {
  return (
    <section className={`min-w-0 rounded-2xl border border-slate-200/80 bg-white shadow-sm dark:border-slate-800 dark:bg-slate-900 ${className}`}>
      {(title || actions) && (
        <header className="flex flex-wrap items-center justify-between gap-2 px-5 pt-4">
          <h2 className="flex items-center gap-2 text-base font-semibold">
            {icon}
            {title}
          </h2>
          <div className="flex flex-wrap items-center gap-2">{actions}</div>
        </header>
      )}
      <div className={bodyClass}>{children}</div>
    </section>
  );
}

type Tone = "default" | "primary" | "danger" | "ghost";

const toneClass: Record<Tone, string> = {
  default:
    "border border-slate-200 bg-white text-slate-700 shadow-sm hover:bg-slate-50 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-200 dark:hover:bg-slate-700",
  primary: "bg-blue-500 text-white shadow-sm hover:bg-blue-600",
  danger:
    "border border-red-200 bg-white text-red-600 hover:bg-red-50 dark:border-red-900 dark:bg-slate-800 dark:text-red-400 dark:hover:bg-red-950",
  ghost: "text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800",
};

export function Button({
  tone = "default",
  small,
  className = "",
  icon,
  children,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { tone?: Tone; small?: boolean; icon?: ReactNode }) {
  return (
    <button
      {...props}
      className={`inline-flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-xl ${
        small ? "px-2.5 py-1 text-xs" : "px-4 py-2 text-sm"
      } font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${toneClass[tone]} ${className}`}
    >
      {icon}
      {children}
    </button>
  );
}

/// Square icon-only button with a tooltip.
export function IconButton({
  title,
  danger,
  className = "",
  children,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { title: string; danger?: boolean }) {
  return (
    <button
      {...props}
      title={title}
      aria-label={title}
      className={`inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-slate-400 transition-colors hover:bg-slate-100 disabled:opacity-40 dark:hover:bg-slate-800 ${
        danger ? "hover:text-red-500" : "hover:text-slate-700 dark:hover:text-slate-200"
      } ${className}`}
    >
      {children}
    </button>
  );
}

export type BadgeTone = "slate" | "green" | "yellow" | "red" | "blue" | "purple";

export function Badge({ tone = "slate", children, icon, title }: { tone?: BadgeTone; children: ReactNode; icon?: ReactNode; title?: string }) {
  const c = {
    slate: "bg-slate-100 text-slate-600 dark:bg-slate-800 dark:text-slate-300",
    green: "bg-green-50 text-green-700 ring-1 ring-green-200 dark:bg-green-950 dark:text-green-300 dark:ring-green-900",
    yellow: "bg-amber-50 text-amber-700 ring-1 ring-amber-200 dark:bg-amber-950 dark:text-amber-300 dark:ring-amber-900",
    red: "bg-red-50 text-red-600 ring-1 ring-red-200 dark:bg-red-950 dark:text-red-300 dark:ring-red-900",
    blue: "bg-blue-50 text-blue-600 ring-1 ring-blue-200 dark:bg-blue-950 dark:text-blue-300 dark:ring-blue-900",
    purple: "bg-violet-50 text-violet-600 ring-1 ring-violet-200 dark:bg-violet-950 dark:text-violet-300 dark:ring-violet-900",
  }[tone];
  return (
    <span title={title} className={`inline-flex items-center gap-1 whitespace-nowrap rounded-md px-1.5 py-0.5 text-xs font-medium ${c}`}>
      {icon}
      {children}
    </span>
  );
}

export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="block text-sm">
      <span className="mb-1 block font-medium text-slate-600 dark:text-slate-300">{label}</span>
      {children}
      {hint && <span className="mt-1 block text-xs text-slate-500">{hint}</span>}
    </label>
  );
}

/// Input styling without a width, for inputs sized by their container.
export const inputBase =
  "rounded-xl border border-slate-200 bg-white px-3 py-2 text-sm outline-none transition focus:border-blue-400 focus:ring-2 focus:ring-blue-100 dark:border-slate-700 dark:bg-slate-950 dark:focus:ring-blue-950";
export const inputClass = `w-full ${inputBase}`;

export function Modal({
  title,
  onClose,
  children,
  wide,
}: {
  title: ReactNode;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-slate-900/40 p-4 pt-12 backdrop-blur-sm" onClick={onClose}>
      <div
        className={`w-full ${wide ? "max-w-3xl" : "max-w-lg"} rounded-2xl bg-white shadow-2xl dark:bg-slate-900`}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label={typeof title === "string" ? title : undefined}
      >
        <header className="flex items-center justify-between px-5 pt-4">
          <h3 className="text-base font-semibold">{title}</h3>
          <IconButton title="close" onClick={onClose}>
            <X className="h-4 w-4" />
          </IconButton>
        </header>
        <div className="space-y-4 p-5">{children}</div>
      </div>
    </div>
  );
}

/// A secret shown exactly once, with a copy button.
export function SecretOnce({ label, value }: { label: string; value: string }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  return (
    <div className="rounded-xl border border-amber-200 bg-amber-50 p-3 dark:border-amber-900 dark:bg-amber-950">
      <div className="mb-1 text-xs text-amber-800 dark:text-amber-200">
        {label}（{t("terminals.secret_once")}）
      </div>
      <div className="flex items-center gap-2">
        <code className="flex-1 break-all text-sm">{value}</code>
        <Button
          small
          onClick={async () => {
            await navigator.clipboard.writeText(value).catch(() => {});
            setCopied(true);
          }}
        >
          {copied ? t("common.copied") : t("common.copy")}
        </Button>
      </div>
    </div>
  );
}

export function ErrorBox({ error }: { error: string | null }) {
  if (!error) return null;
  return (
    <div className="rounded-xl border border-red-200 bg-red-50 p-3 text-sm text-red-700 dark:border-red-900 dark:bg-red-950 dark:text-red-200">
      {error}
    </div>
  );
}

/// Remaining-quota bar: `remaining` 0–1 (null = unknown), with the reset
/// countdown.
export function QuotaBar({
  label,
  remaining,
  resetAt,
  now,
  compact,
}: {
  label: string;
  remaining: number | null;
  resetAt?: number | null;
  now: number;
  compact?: boolean;
}) {
  const { t } = useI18n();
  const pct = remaining === null ? null : Math.round(remaining * 100);
  const color = pct === null ? "bg-slate-300 dark:bg-slate-700" : pct >= 50 ? "bg-emerald-500" : pct >= 20 ? "bg-amber-500" : "bg-red-500";
  const text = pct === null ? "text-slate-400" : pct >= 50 ? "text-emerald-600" : pct >= 20 ? "text-amber-600" : "text-red-600";
  return (
    <div className={compact ? "text-[11px]" : "text-xs"}>
      <div className="mb-1 flex items-center justify-between gap-2 text-slate-500">
        <span className="truncate">{label}</span>
        <span className="flex shrink-0 items-center gap-2">
          {resetAt && resetAt > now && <span className="font-mono text-slate-400">{t("quota.reset_in", { t: fmtCountdown(resetAt - now) })}</span>}
          <span className={`font-semibold ${text}`}>{pct === null ? "—" : `${pct}%`}</span>
        </span>
      </div>
      <div className={`${compact ? "h-1.5" : "h-2"} w-full overflow-hidden rounded-full bg-slate-100 dark:bg-slate-800`}>
        <div className={`h-full rounded-full ${color} transition-all`} style={{ width: `${pct ?? 0}%` }} />
      </div>
    </div>
  );
}

/// Segmented control (filter tabs, view toggles).
export function Segmented<K extends string>({
  value,
  options,
  onChange,
}: {
  value: K;
  options: { key: K; label: ReactNode; title?: string }[];
  onChange: (k: K) => void;
}) {
  return (
    <div className="inline-flex shrink-0 rounded-xl bg-slate-100 p-1 dark:bg-slate-800">
      {options.map((o) => (
        <button
          key={o.key}
          title={o.title}
          onClick={() => onChange(o.key)}
          className={`inline-flex items-center gap-1 whitespace-nowrap rounded-lg px-3 py-1.5 text-sm transition-colors ${
            value === o.key
              ? "bg-white font-medium text-slate-900 shadow-sm dark:bg-slate-950 dark:text-white"
              : "text-slate-500 hover:text-slate-800 dark:hover:text-slate-200"
          }`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Pagination({
  page,
  pageSize,
  total,
  onChange,
}: {
  page: number;
  pageSize: number;
  total: number;
  onChange: (p: number) => void;
}) {
  const { t } = useI18n();
  const pages = Math.max(1, Math.ceil(total / pageSize));
  if (total === 0) return null;
  const from = (page - 1) * pageSize + 1;
  const to = Math.min(total, page * pageSize);
  const nums = Array.from({ length: pages }, (_, i) => i + 1).filter((p) => p === 1 || p === pages || Math.abs(p - page) <= 1);
  return (
    <div className="flex flex-wrap items-center justify-between gap-2 text-sm text-slate-500">
      <span>{t("common.page_info", { from, to, total })}</span>
      <div className="flex items-center gap-1">
        <IconButton title={t("common.prev")} disabled={page <= 1} onClick={() => onChange(page - 1)}>
          <ChevronLeft className="h-4 w-4" />
        </IconButton>
        {nums.map((p, i) => (
          <span key={p} className="flex items-center">
            {i > 0 && p - nums[i - 1] > 1 && <span className="px-1">…</span>}
            <button
              onClick={() => onChange(p)}
              className={`h-8 min-w-8 rounded-lg px-2 ${p === page ? "bg-blue-500 font-medium text-white" : "hover:bg-slate-100 dark:hover:bg-slate-800"}`}
            >
              {p}
            </button>
          </span>
        ))}
        <IconButton title={t("common.next")} disabled={page >= pages} onClick={() => onChange(page + 1)}>
          <ChevronRight className="h-4 w-4" />
        </IconButton>
      </div>
    </div>
  );
}

export function StatCard({
  icon,
  iconBg,
  value,
  label,
  hint,
  hintTone = "slate",
}: {
  icon: ReactNode;
  iconBg: string;
  value: ReactNode;
  label: string;
  hint?: ReactNode;
  hintTone?: "slate" | "green" | "red";
}) {
  const hintColor = { slate: "text-slate-400", green: "text-emerald-600", red: "text-red-500" }[hintTone];
  return (
    <div className="rounded-2xl border border-slate-200/80 bg-white p-5 shadow-sm dark:border-slate-800 dark:bg-slate-900">
      <div className={`mb-3 inline-flex h-9 w-9 items-center justify-center rounded-xl ${iconBg}`}>{icon}</div>
      <div className="text-2xl font-bold tracking-tight">{value}</div>
      <div className="mt-0.5 text-sm text-slate-500">{label}</div>
      {hint && <div className={`mt-1 text-xs ${hintColor}`}>{hint}</div>}
    </div>
  );
}

/// Loads data on mount and on `reload()`, tracking loading / error state.
export function useLoad<T>(load: () => Promise<T>, deps: unknown[] = []) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const run = useCallback(load, deps);
  // Only the latest call may set state: a slow response for old deps must
  // not overwrite a newer one.
  const seq = useRef(0);
  const reload = useCallback(async () => {
    const mine = ++seq.current;
    setLoading(true);
    try {
      const v = await run();
      if (mine !== seq.current) return;
      setData(v);
      setError(null);
    } catch (e) {
      if (mine !== seq.current) return;
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (mine === seq.current) setLoading(false);
    }
  }, [run]);
  useEffect(() => {
    void reload();
  }, [reload]);
  return { data, error, loading, reload, setError };
}

/// Wraps an async action with busy / error state for buttons.
export function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = useCallback(async <T,>(fn: () => Promise<T>): Promise<T | undefined> => {
    setBusy(true);
    setError(null);
    try {
      return await fn();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return undefined;
    } finally {
      setBusy(false);
    }
  }, []);
  return { busy, error, run, setError };
}

/// On/off toggle.
export function Switch({ checked, onChange, disabled, label }: { checked: boolean; onChange: (v: boolean) => void; disabled?: boolean; label?: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors disabled:opacity-50 ${checked ? "bg-blue-500" : "bg-slate-300 dark:bg-slate-700"}`}
    >
      <span className={`inline-block h-5 w-5 rounded-full bg-white shadow transition-transform ${checked ? "translate-x-5" : "translate-x-0.5"}`} />
    </button>
  );
}

/// A labelled row with a control on the right (settings lists).
export function SettingRow({ title, hint, children }: { title: ReactNode; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 py-3">
      <div className="min-w-0">
        <div className="text-sm font-medium">{title}</div>
        {hint && <div className="text-xs text-slate-500">{hint}</div>}
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </div>
  );
}
