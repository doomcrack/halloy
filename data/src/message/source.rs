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

/// A message the app produced about itself rather than received.
///
/// `Module` stays [`Copy`] like its sibling — the source is passed by value
/// through `message_view`, `scroll_view` and `context_menu` — so it carries
/// only the severity. Which module a line belongs to is carried by
/// [`history::Kind::Module`](crate::history::Kind), which every read path
/// already keys on; duplicating the id here would buy nothing and cost the
/// `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Internal {
    Logs(log::Level),
    Module(crate::module::log::Level),
}
