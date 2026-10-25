//! Per-account usage of the Pro/Max rate-limit windows (5 hours, 7 days).
//!
//! Captured passively from the `anthropic-ratelimit-unified-*` headers on
//! every upstream response, and optionally polled from a usage endpoint.
//! Neither source is publicly documented, so parsing is lenient: missing
//! fields keep their previous value.

use serde::Serialize;
use serde_json::Value;

/// A (possibly partial) reading. `None` fields weren't in the source.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct QuotaSnapshot {
    /// Fraction of the window used, 0.0–1.0.
    pub five_hour_utilization: Option<f64>,
    /// Unix seconds.
    pub five_hour_reset: Option<i64>,
    pub seven_day_utilization: Option<f64>,
    pub seven_day_reset: Option<i64>,
    /// `allowed`, `allowed_warning`, `rejected`, … as reported upstream.
    pub status: Option<String>,
}

impl QuotaSnapshot {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The stored, merged view of an account's quota.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Quota {
    pub five_hour_utilization: Option<f64>,
    pub five_hour_reset: Option<i64>,
    pub seven_day_utilization: Option<f64>,
    pub seven_day_reset: Option<i64>,
    pub status: String,
    pub updated_at: i64,
}

impl Quota {
    /// Highest utilization across windows that haven't reset yet at `now`.
    pub fn effective_utilization(&self, now: i64) -> f64 {
        let live = |u: Option<f64>, reset: Option<i64>| match (u, reset) {
            (Some(u), Some(r)) if r > now => u,
            (Some(u), None) => u,
            _ => 0.0,
        };
        live(self.five_hour_utilization, self.five_hour_reset)
            .max(live(self.seven_day_utilization, self.seven_day_reset))
    }
}

/// Utilization may come as a fraction (0.42) or a percentage (42); values
/// above 1.5 are taken as percentages.
fn fraction(v: f64) -> f64 {
    let f = if v > 1.5 { v / 100.0 } else { v };
    f.clamp(0.0, 1.0)
}

/// Unix seconds, or an RFC 3339 timestamp.
fn timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    s.parse::<i64>().ok().or_else(|| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.timestamp())
    })
}

/// Reads the unified rate-limit headers of an upstream response.
pub fn from_headers(h: &reqwest::header::HeaderMap) -> QuotaSnapshot {
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok());
    let util = |k: &str| {
        get(k)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(fraction)
    };
    QuotaSnapshot {
        five_hour_utilization: util("anthropic-ratelimit-unified-5h-utilization"),
        five_hour_reset: get("anthropic-ratelimit-unified-5h-reset").and_then(timestamp),
        seven_day_utilization: util("anthropic-ratelimit-unified-7d-utilization"),
        seven_day_reset: get("anthropic-ratelimit-unified-7d-reset").and_then(timestamp),
        status: get("anthropic-ratelimit-unified-status").map(|s| s.trim().to_string()),
    }
}

/// Reads a usage-endpoint body shaped like
/// `{"five_hour": {"utilization": 42, "resets_at": "…"}, "seven_day": {…}}`.
pub fn from_usage_body(v: &Value) -> QuotaSnapshot {
    let window = |name: &str| {
        let w = &v[name];
        (
            w["utilization"].as_f64().map(fraction),
            w["resets_at"]
                .as_str()
                .and_then(timestamp)
                .or_else(|| w["resets_at"].as_i64()),
        )
    };
    let (fu, fr) = window("five_hour");
    let (su, sr) = window("seven_day");
    QuotaSnapshot {
        five_hour_utilization: fu,
        five_hour_reset: fr,
        seven_day_utilization: su,
        seven_day_reset: sr,
        status: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn headers_fraction_and_percent() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            "anthropic-ratelimit-unified-5h-utilization",
            "0.42".parse().unwrap(),
        );
        h.insert(
            "anthropic-ratelimit-unified-5h-reset",
            "1800000000".parse().unwrap(),
        );
        h.insert(
            "anthropic-ratelimit-unified-7d-utilization",
            "18".parse().unwrap(),
        );
        h.insert(
            "anthropic-ratelimit-unified-7d-reset",
            "2027-01-01T00:00:00Z".parse().unwrap(),
        );
        h.insert(
            "anthropic-ratelimit-unified-status",
            "allowed".parse().unwrap(),
        );
        let q = from_headers(&h);
        assert_eq!(q.five_hour_utilization, Some(0.42));
        assert_eq!(q.five_hour_reset, Some(1_800_000_000));
        assert!((q.seven_day_utilization.unwrap() - 0.18).abs() < 1e-9);
        assert_eq!(q.seven_day_reset, Some(1_798_761_600));
        assert_eq!(q.status.as_deref(), Some("allowed"));
        assert!(from_headers(&reqwest::header::HeaderMap::new()).is_empty());
    }

    #[test]
    fn usage_body() {
        let q = from_usage_body(&json!({
            "five_hour": {"utilization": 87.0, "resets_at": "2027-01-01T00:00:00+00:00"},
            "seven_day": {"utilization": 12, "resets_at": null}
        }));
        assert!((q.five_hour_utilization.unwrap() - 0.87).abs() < 1e-9);
        assert_eq!(q.five_hour_reset, Some(1_798_761_600));
        assert!((q.seven_day_utilization.unwrap() - 0.12).abs() < 1e-9);
        assert_eq!(q.seven_day_reset, None);
    }

    #[test]
    fn effective_utilization_ignores_reset_windows() {
        let q = Quota {
            five_hour_utilization: Some(0.99),
            five_hour_reset: Some(100),
            seven_day_utilization: Some(0.3),
            seven_day_reset: Some(1000),
            ..Default::default()
        };
        assert!((q.effective_utilization(50) - 0.99).abs() < 1e-9);
        // The 5h window reset at 100: only the 7d reading counts.
        assert!((q.effective_utilization(200) - 0.3).abs() < 1e-9);
        assert_eq!(q.effective_utilization(2000), 0.0);
    }
}
