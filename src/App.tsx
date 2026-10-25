import { useEffect, useState } from "react";
import { getStatus, isTauri, type Status } from "./api";

export default function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const load = () =>
      getStatus()
        .then((s) => {
          if (!cancelled) {
            setStatus(s);
            setError(null);
          }
        })
        .catch((e) => !cancelled && setError(String(e)));
    load();
    const timer = setInterval(load, 5000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  return (
    <main className="mx-auto max-w-3xl px-4 py-8">
      <header className="mb-6 flex items-baseline justify-between">
        <h1 className="text-2xl font-semibold">CC Manager</h1>
        <span className="text-sm text-slate-500">{isTauri ? "桌面模式" : "Web 模式"}</span>
      </header>

      {error && (
        <div className="mb-4 rounded-md border border-red-300 bg-red-50 p-3 text-sm text-red-800 dark:border-red-800 dark:bg-red-950 dark:text-red-200">
          无法连接服务：{error}
        </div>
      )}

      <section className="grid gap-4 sm:grid-cols-3">
        <Stat label="服务状态" value={status ? "运行中" : error ? "未连接" : "…"} />
        <Stat label="监听地址" value={status?.listen ?? "—"} mono />
        <Stat label="上游账号" value={status ? String(status.accounts) : "—"} />
      </section>

      <section className="mt-6 rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900">
        <h2 className="mb-2 text-sm font-medium text-slate-500">当前上游凭据</h2>
        <p className="break-all font-mono text-sm">{status?.upstream ?? "—"}</p>
      </section>

      <footer className="mt-8 text-xs text-slate-400">v{status?.version ?? "?"}</footer>
    </main>
  );
}

function Stat({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900">
      <div className="text-xs text-slate-500">{label}</div>
      <div className={`mt-1 text-lg ${mono ? "font-mono text-base" : "font-semibold"}`}>{value}</div>
    </div>
  );
}
