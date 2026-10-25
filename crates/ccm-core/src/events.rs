//! Account health changes, broadcast for the desktop app (notifications,
//! tray) and anything else that wants to react.

use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CoreEvent {
    /// The account's refresh token was rejected; it needs a new login.
    NeedsReauth { account_id: i64, reason: String },
    /// The scheduler parked the account (429 / 403).
    CoolingDown {
        account_id: i64,
        seconds: i64,
        reason: String,
    },
    /// No eligible account is free; requests are being turned away.
    AllCooling { retry_after: i64 },
    /// Some account's quota snapshot changed.
    QuotaUpdated { account_id: i64 },
}

/// Cheap to clone; sending never blocks and is dropped without listeners.
#[derive(Clone)]
pub struct Events(broadcast::Sender<CoreEvent>);

impl Default for Events {
    fn default() -> Self {
        Self(broadcast::channel(64).0)
    }
}

impl Events {
    pub fn send(&self, e: CoreEvent) {
        let _ = self.0.send(e);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.0.subscribe()
    }
}
