import { invoke } from "@tauri-apps/api/core";
import { Maximize2 } from "lucide-react";
import { useEffect } from "react";
import { displayName, remaining, resetAt } from "../accounts";
import { get, isTauri } from "../api";
import { useI18n } from "../i18n";
import type { AccountsResponse, ServiceStatus } from "../types";
import { Badge, IconButton, QuotaBar, useLoad } from "../ui";
import { StatusBadge } from "./Accounts";

/// The desktop mini window: current account, its quota, proxy state.
export default function Mini() {
  const { t } = useI18n();
  const accounts = useLoad(() => get<AccountsResponse>("/accounts"));
  const svc = useLoad(() => get<ServiceStatus>("/service"));
  useEffect(() => {
    const id = setInterval(() => {
      void accounts.reload();
      void svc.reload();
    }, 15_000);
    return () => clearInterval(id);
  }, [accounts.reload, svc.reload]);
  const now = accounts.data?.now ?? Date.now() / 1000;
  const cur = accounts.data?.accounts.find((a) => a.id === accounts.data?.recommended);
  const healthy = (accounts.data?.accounts ?? []).filter((a) => a.enabled && a.status === "active" && a.cooldown_until <= now).length;
  return (
    <div className="flex min-h-screen flex-col gap-3 bg-white p-4 text-sm dark:bg-slate-900">
      <div className="flex items-center gap-2">
        {svc.data?.running ? <Badge tone="green">● {t("connect.running")}</Badge> : <Badge tone="red">{t("proxy.stopped")}</Badge>}
        <span className="truncate font-mono text-xs text-slate-500">{svc.data?.bound ?? ""}</span>
        {isTauri && (
          <IconButton className="ml-auto" title={t("mini.open_main")} onClick={() => invoke("show_main_window")}>
            <Maximize2 className="h-4 w-4" />
          </IconButton>
        )}
      </div>
      {cur ? (
        <>
          <div className="flex min-w-0 items-center gap-2">
            <span className="truncate font-semibold">{displayName(cur)}</span>
            <StatusBadge a={cur} now={now} />
          </div>
          <QuotaBar label={t("quota.five_hour")} remaining={remaining(cur.quota, "5h", now)} resetAt={resetAt(cur.quota, "5h")} now={now} />
          <QuotaBar label={t("quota.seven_day")} remaining={remaining(cur.quota, "7d", now)} resetAt={resetAt(cur.quota, "7d")} now={now} />
        </>
      ) : (
        <p className="text-slate-500">{t("dashboard.no_current")}</p>
      )}
      <div className="mt-auto text-xs text-slate-400">{t("mini.healthy", { n: healthy, total: accounts.data?.accounts.length ?? 0 })}</div>
    </div>
  );
}
