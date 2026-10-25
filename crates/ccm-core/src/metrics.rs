//! Prometheus text exposition for `GET /metrics` (admin token required).

use std::fmt::Write;

use anyhow::Result;

use crate::Core;

fn label(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

impl Core {
    pub fn metrics(&self) -> Result<String> {
        let mut out = String::new();
        let now = crate::now_unix();
        let accounts = self.accounts.list()?;
        let quotas = self.pool.quotas()?;
        let name = |id: i64| {
            accounts.iter().find(|a| a.id == id).map_or_else(
                || {
                    if id == 0 {
                        "none".into()
                    } else {
                        format!("#{id}")
                    }
                },
                |a| a.label.clone(),
            )
        };
        let c = self.recorder.counters();

        writeln!(out, "# HELP ccm_requests_total Messages requests by upstream account and HTTP status (including failover attempts).")?;
        writeln!(out, "# TYPE ccm_requests_total counter")?;
        for ((account, status), n) in &c.requests {
            writeln!(
                out,
                "ccm_requests_total{{account_id=\"{account}\",account=\"{}\",status=\"{status}\"}} {n}",
                label(&name(*account))
            )?;
        }
        writeln!(
            out,
            "# HELP ccm_tokens_total Tokens by upstream account and kind."
        )?;
        writeln!(out, "# TYPE ccm_tokens_total counter")?;
        for (account, t) in &c.tokens {
            for (kind, v) in ["input", "output", "cache_read", "cache_creation"]
                .iter()
                .zip(t)
            {
                writeln!(
                    out,
                    "ccm_tokens_total{{account_id=\"{account}\",account=\"{}\",kind=\"{kind}\"}} {v}",
                    label(&name(*account))
                )?;
            }
        }
        writeln!(out, "# HELP ccm_stats_dropped_total Usage events dropped because the writer queue was full.")?;
        writeln!(out, "# TYPE ccm_stats_dropped_total counter")?;
        writeln!(out, "ccm_stats_dropped_total {}", c.dropped)?;

        writeln!(
            out,
            "# HELP ccm_account_up 1 when the account is enabled, active and not cooling down."
        )?;
        writeln!(out, "# TYPE ccm_account_up gauge")?;
        writeln!(
            out,
            "# HELP ccm_account_cooldown_seconds Remaining cooldown."
        )?;
        writeln!(out, "# TYPE ccm_account_cooldown_seconds gauge")?;
        writeln!(
            out,
            "# HELP ccm_account_quota_utilization Last known usage of a rate-limit window (0-1)."
        )?;
        writeln!(out, "# TYPE ccm_account_quota_utilization gauge")?;
        for a in &accounts {
            let l = format!("account_id=\"{}\",account=\"{}\"", a.id, label(&a.label));
            let up = a.enabled
                && a.status == crate::accounts::STATUS_ACTIVE
                && a.cooldown_left(now) == 0;
            writeln!(
                out,
                "ccm_account_up{{{l},status=\"{}\"}} {}",
                label(&a.status),
                u8::from(up)
            )?;
            writeln!(
                out,
                "ccm_account_cooldown_seconds{{{l}}} {}",
                a.cooldown_left(now)
            )?;
            if let Some(q) = quotas.get(&a.id) {
                for (window, v) in [
                    ("5h", q.five_hour_utilization),
                    ("7d", q.seven_day_utilization),
                ] {
                    if let Some(v) = v {
                        writeln!(
                            out,
                            "ccm_account_quota_utilization{{{l},window=\"{window}\"}} {v}"
                        )?;
                    }
                }
            }
        }
        Ok(out)
    }
}
