use serde::{Deserialize, Serialize};

use crate::address::Address;
use crate::log;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Peer(Address),
    Yourself,
    Status(StatusKind),
    Internal(Internal),
}

impl Source {
    pub fn peer(&self) -> Option<&Address> {
        match self {
            Source::Peer(address) => Some(address),
            Source::Yourself | Source::Status(_) | Source::Internal(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StatusKind {
    NewConversation,
    MemberChange,
    SendFailed,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Internal {
    Logs(log::Level),
}
