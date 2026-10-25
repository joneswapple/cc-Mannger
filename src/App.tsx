import { useEffect, useState } from "react";
import { ApiError, get, getToken, isTauri, setToken } from "./api";
import Accounts from "./pages/Accounts";
import Connect from "./pages/Connect";
import Dashboard from "./pages/Dashboard";
import Invites from "./pages/Invites";
import Settings from "./pages/Settings";
import Usage from "./pages/Usage";
import Users from "./pages/Users";
import { Button, ErrorBox, inputClass } from "./ui";

const PAGES = {
  dashboard: { label: "概览", component: Dashboard },
  accounts: { label: "上游账号", component: Accounts },
  users: { label: "终端", component: Users },
  invites: { label: "邀请码", component: Invites },
  usage: { label: "用量", component: Usage },
  settings: { label: "设置", component: Settings },
  connect: { label: "接入", component: Connect },
} as const;

type PageKey = keyof typeof PAGES;

function initialPage(): PageKey {
  const h = location.hash.replace("#", "");
  return h in PAGES ? (h as PageKey) : "dashboard";
}

export default function App() {
  const [authed, setAuthed] = useState<boolean | null>(isTauri ? true : null);
  const [page, setPage] = useState<PageKey>(initialPage);

  useEffect(() => {
    if (isTauri) return;
    if (!getToken()) {
      setAuthed(false);
      return;
    }
    get("/status")
      .then(() => setAuthed(true))
      .catch((e) => setAuthed(!(e instanceof ApiError && e.status === 401)));
  }, []);

  useEffect(() => {
    const onHash = () => setPage(initialPage());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  if (authed === null) return null;
  if (!authed) return <Login onLogin={() => setAuthed(true)} />;

  const Page = PAGES[page].component;
  return (
    <div className="flex min-h-screen flex-col md:flex-row">
      <nav className="flex shrink-0 gap-1 overflow-x-auto border-b border-slate-200 bg-white p-2 md:w-48 md:flex-col md:border-b-0 md:border-r dark:border-slate-800 dark:bg-slate-900">
        <div className="hidden px-2 py-3 text-lg font-semibold md:block">CC Manager</div>
        {(Object.keys(PAGES) as PageKey[]).map((k) => (
          <a
            key={k}
            href={`#${k}`}
            className={`whitespace-nowrap rounded-md px-3 py-2 text-sm ${
              page === k ? "bg-orange-100 font-medium text-orange-900 dark:bg-orange-950 dark:text-orange-200" : "hover:bg-slate-100 dark:hover:bg-slate-800"
            }`}
          >
            {PAGES[k].label}
          </a>
        ))}
        {!isTauri && (
          <button
            className="mt-auto whitespace-nowrap rounded-md px-3 py-2 text-left text-sm text-slate-500 hover:bg-slate-100 dark:hover:bg-slate-800"
            onClick={() => {
              setToken("");
              setAuthed(false);
            }}
          >
            退出登录
          </button>
        )}
      </nav>
      <main className="min-w-0 flex-1 p-4 md:p-6">
        <h1 className="mb-4 text-xl font-semibold">{PAGES[page].label}</h1>
        <Page />
      </main>
    </div>
  );
}

function Login({ onLogin }: { onLogin: () => void }) {
  const [token, setTokenInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  return (
    <div className="flex min-h-screen items-center justify-center p-4">
      <form
        className="w-full max-w-sm space-y-3 rounded-lg border border-slate-200 bg-white p-6 dark:border-slate-800 dark:bg-slate-900"
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          setToken(token.trim());
          try {
            await get("/status");
            onLogin();
          } catch (err) {
            setToken("");
            setError(err instanceof ApiError && err.status === 401 ? "管理员令牌不正确" : String(err));
          } finally {
            setBusy(false);
          }
        }}
      >
        <h1 className="text-lg font-semibold">CC Manager</h1>
        <p className="text-sm text-slate-500">
          输入管理员令牌（首次启动 <code>ccm-server</code> 时打印；忘了可以运行 <code>ccm-server admin reset-token</code>）。
        </p>
        <ErrorBox error={error} />
        <input className={`${inputClass} font-mono`} type="password" value={token} onChange={(e) => setTokenInput(e.target.value)} autoFocus />
        <Button tone="primary" type="submit" disabled={busy || !token.trim()} className="w-full">
          登录
        </Button>
      </form>
    </div>
  );
}
