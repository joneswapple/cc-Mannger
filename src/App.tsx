import { LogOut, Monitor as MonitorIcon, Moon, Sun } from "lucide-react";
import { useEffect, useState } from "react";
import { ApiError, get, getToken, isTauri, setToken } from "./api";
import { LangProvider, useI18n } from "./i18n";
import Accounts from "./pages/Accounts";
import Dashboard from "./pages/Dashboard";
import Monitor from "./pages/Monitor";
import Mini from "./pages/Mini";
import Proxy from "./pages/Proxy";
import Security from "./pages/Security";
import Settings from "./pages/Settings";
import Terminals from "./pages/Terminals";
import Usage from "./pages/Usage";
import { useTheme } from "./theme";
import { ToastProvider } from "./toast";
import { Button, ErrorBox, inputClass } from "./ui";

const PAGES = {
  dashboard: Dashboard,
  accounts: Accounts,
  proxy: Proxy,
  monitor: Monitor,
  terminals: Terminals,
  usage: Usage,
  security: Security,
  settings: Settings,
} as const;

export type PageKey = keyof typeof PAGES;

function pageFromHash(): PageKey {
  const raw = location.hash.replace("#", "").split("?")[0];
  // Older links used #connect for the proxy page.
  const h = raw === "connect" ? "proxy" : raw;
  return h in PAGES ? (h as PageKey) : "dashboard";
}

export function navigate(page: PageKey) {
  location.hash = page;
}

export default function App() {
  return (
    <LangProvider>
      <ToastProvider>
        <Shell />
      </ToastProvider>
    </LangProvider>
  );
}

function Shell() {
  const [authed, setAuthed] = useState<boolean | null>(isTauri ? true : null);
  const [page, setPage] = useState<PageKey>(pageFromHash);
  const { t, toggle } = useI18n();
  const theme = useTheme();

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
    const onHash = () => setPage(pageFromHash());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  if (authed === null) return null;
  if (!authed) return <Login onLogin={() => setAuthed(true)} />;
  // The desktop app's always-on-top mini window.
  if (location.hash.startsWith("#mini")) return <Mini />;

  const Page = PAGES[page];
  const ThemeIcon = theme.pref === "light" ? Sun : theme.pref === "dark" ? Moon : MonitorIcon;
  const roundBtn =
    "inline-flex h-10 w-10 items-center justify-center rounded-full bg-white text-sm font-semibold text-slate-600 shadow-sm ring-1 ring-slate-200 hover:bg-slate-50 dark:bg-slate-900 dark:text-slate-300 dark:ring-slate-800 dark:hover:bg-slate-800";
  return (
    <div className="min-h-screen">
      <header className="sticky top-0 z-40 border-b border-slate-200/60 bg-slate-100/80 backdrop-blur dark:border-slate-800/60 dark:bg-slate-950/80">
        <div className="mx-auto flex max-w-7xl items-center gap-3 px-4 py-3 md:px-6">
          <a href="#dashboard" className="flex shrink-0 items-center gap-2">
            <img src="/icon.png" alt="" className="h-8 w-8 rounded-lg" onError={(e) => (e.currentTarget.style.display = "none")} />
            <span className="hidden text-lg font-bold tracking-tight sm:inline">CC Manager</span>
          </a>
          <nav className="mx-auto flex min-w-0 overflow-x-auto rounded-full bg-white p-1 shadow-sm ring-1 ring-slate-200 dark:bg-slate-900 dark:ring-slate-800">
            {(Object.keys(PAGES) as PageKey[]).map((k) => (
              <a
                key={k}
                href={`#${k}`}
                className={`whitespace-nowrap rounded-full px-4 py-1.5 text-sm transition-colors ${
                  page === k
                    ? "bg-slate-900 font-medium text-white dark:bg-white dark:text-slate-900"
                    : "text-slate-600 hover:text-slate-900 dark:text-slate-400 dark:hover:text-white"
                }`}
              >
                {t(`nav.${k}`)}
              </a>
            ))}
          </nav>
          <div className="flex shrink-0 items-center gap-2">
            <button className={roundBtn} onClick={theme.next} title={t(`nav.theme_${theme.pref}`)} aria-label={t(`nav.theme_${theme.pref}`)}>
              <ThemeIcon className="h-4 w-4" />
            </button>
            <button className={roundBtn} onClick={toggle} title="Language">
              {t("nav.lang")}
            </button>
            {!isTauri && (
              <button
                className={roundBtn}
                title={t("nav.logout")}
                aria-label={t("nav.logout")}
                onClick={() => {
                  setToken("");
                  setAuthed(false);
                }}
              >
                <LogOut className="h-4 w-4" />
              </button>
            )}
          </div>
        </div>
      </header>
      <main className="mx-auto max-w-7xl px-4 py-6 md:px-6">
        <Page />
      </main>
    </div>
  );
}

function Login({ onLogin }: { onLogin: () => void }) {
  const { t } = useI18n();
  const [token, setTokenInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  return (
    <div className="flex min-h-screen items-center justify-center p-4">
      <form
        className="w-full max-w-sm space-y-4 rounded-2xl border border-slate-200 bg-white p-6 shadow-sm dark:border-slate-800 dark:bg-slate-900"
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          setToken(token.trim());
          try {
            await get("/status");
            onLogin();
          } catch (err) {
            setToken("");
            setError(err instanceof ApiError && err.status === 401 ? t("login.wrong") : String(err));
          } finally {
            setBusy(false);
          }
        }}
      >
        <h1 className="text-xl font-bold">{t("login.title")}</h1>
        <p className="text-sm text-slate-500">{t("login.hint")}</p>
        <ErrorBox error={error} />
        <input className={`${inputClass} font-mono`} type="password" value={token} onChange={(e) => setTokenInput(e.target.value)} autoFocus />
        <Button tone="primary" type="submit" disabled={busy || !token.trim()} className="w-full">
          {t("login.submit")}
        </Button>
      </form>
    </div>
  );
}
