import { useCallback, useEffect, useState, type ReactNode } from "react";

export function Card({ title, actions, children }: { title?: ReactNode; actions?: ReactNode; children: ReactNode }) {
  return (
    <section className="rounded-lg border border-slate-200 bg-white dark:border-slate-800 dark:bg-slate-900">
      {(title || actions) && (
        <header className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-200 px-4 py-3 dark:border-slate-800">
          <h2 className="text-sm font-semibold">{title}</h2>
          <div className="flex flex-wrap gap-2">{actions}</div>
        </header>
      )}
      <div className="p-4">{children}</div>
    </section>
  );
}

type Tone = "default" | "primary" | "danger" | "ghost";

const toneClass: Record<Tone, string> = {
  default:
    "border border-slate-300 bg-white hover:bg-slate-50 dark:border-slate-700 dark:bg-slate-800 dark:hover:bg-slate-700",
  primary: "bg-orange-600 text-white hover:bg-orange-700",
  danger: "border border-red-300 text-red-700 hover:bg-red-50 dark:border-red-800 dark:text-red-300 dark:hover:bg-red-950",
  ghost: "text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800",
};

export function Button({
  tone = "default",
  small,
  className = "",
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { tone?: Tone; small?: boolean }) {
  return (
    <button
      {...props}
      className={`shrink-0 whitespace-nowrap rounded-md ${small ? "px-2 py-1 text-xs" : "px-3 py-1.5 text-sm"} font-medium disabled:opacity-50 ${toneClass[tone]} ${className}`}
    />
  );
}

export function Badge({ tone = "slate", children }: { tone?: "slate" | "green" | "yellow" | "red" | "blue"; children: ReactNode }) {
  const c = {
    slate: "bg-slate-100 text-slate-700 dark:bg-slate-800 dark:text-slate-300",
    green: "bg-green-100 text-green-800 dark:bg-green-950 dark:text-green-300",
    yellow: "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300",
    red: "bg-red-100 text-red-800 dark:bg-red-950 dark:text-red-300",
    blue: "bg-sky-100 text-sky-800 dark:bg-sky-950 dark:text-sky-300",
  }[tone];
  return <span className={`inline-flex items-center rounded px-1.5 py-0.5 text-xs font-medium ${c}`}>{children}</span>;
}

export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="block text-sm">
      <span className="mb-1 block text-slate-600 dark:text-slate-400">{label}</span>
      {children}
      {hint && <span className="mt-1 block text-xs text-slate-500">{hint}</span>}
    </label>
  );
}

/// Input styling without a width, for inputs sized by their container.
export const inputBase =
  "rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm dark:border-slate-700 dark:bg-slate-950";
export const inputClass = `w-full ${inputBase}`;

export function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/40 p-4 pt-16" onClick={onClose}>
      <div
        className="w-full max-w-lg rounded-lg bg-white shadow-xl dark:bg-slate-900"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label={title}
      >
        <header className="flex items-center justify-between border-b border-slate-200 px-4 py-3 dark:border-slate-800">
          <h3 className="font-semibold">{title}</h3>
          <button onClick={onClose} className="text-slate-500 hover:text-slate-800" aria-label="关闭">
            ✕
          </button>
        </header>
        <div className="space-y-3 p-4">{children}</div>
      </div>
    </div>
  );
}

/// A secret shown exactly once, with a copy button.
export function SecretOnce({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="rounded-md border border-amber-300 bg-amber-50 p-3 dark:border-amber-800 dark:bg-amber-950">
      <div className="mb-1 text-xs text-amber-800 dark:text-amber-200">{label}（只显示这一次，请立即保存）</div>
      <div className="flex items-center gap-2">
        <code className="flex-1 break-all text-sm">{value}</code>
        <Button
          small
          onClick={async () => {
            await navigator.clipboard.writeText(value).catch(() => {});
            setCopied(true);
          }}
        >
          {copied ? "已复制" : "复制"}
        </Button>
      </div>
    </div>
  );
}

export function ErrorBox({ error }: { error: string | null }) {
  if (!error) return null;
  return (
    <div className="rounded-md border border-red-300 bg-red-50 p-2 text-sm text-red-800 dark:border-red-800 dark:bg-red-950 dark:text-red-200">
      {error}
    </div>
  );
}

export function Meter({ value, label }: { value: number | null | undefined; label: string }) {
  const pct = value === null || value === undefined ? null : Math.min(100, Math.max(0, value * 100));
  const color = pct === null ? "bg-slate-300" : pct >= 90 ? "bg-red-500" : pct >= 70 ? "bg-amber-500" : "bg-green-500";
  return (
    <div className="text-xs">
      <div className="mb-0.5 flex justify-between text-slate-500">
        <span>{label}</span>
        <span>{pct === null ? "—" : `${Math.round(pct)}%`}</span>
      </div>
      <div className="h-1.5 w-full rounded bg-slate-200 dark:bg-slate-800">
        <div className={`h-1.5 rounded ${color}`} style={{ width: `${pct ?? 0}%` }} />
      </div>
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
  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setData(await run());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
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
