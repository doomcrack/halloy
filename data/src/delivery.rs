//! Delivery-channel readiness — the gate for every mutating chat action —
//! plus the ephemeral per-run identity the module hands out on `init`.

pub use logos_chat::DeliveryState;

use crate::address::Address;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Delivery {
    pub status: DeliveryState,
    pub detail: String,
}

impl Delivery {
    /// Mutating actions are only allowed while delivery is online.
    pub fn can_act(&self) -> bool {
        self.status.is_online()
    }
}

/// Identity is ephemeral upstream — a new address per `init` — so this
/// lives for one app run and is never persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub address: Address,
    pub installation_name: Option<String>,
}
