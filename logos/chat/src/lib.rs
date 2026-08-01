//! Typed client for logos-chat-module (contract v0.2.0,
//! `rust-lib/chat_module.lidl`) plus the backend session state machine the
//! app consumes as an iced-compatible stream.

pub mod client;
pub mod config;
pub mod error;
pub mod event;
pub mod session;
pub mod types;

pub use client::ChatClient;
pub use config::BackendConfig;
pub use error::{ActionError, BackendError, ChatError, LoadKind};
pub use event::ChatEvent;
pub use session::{Control, Driver, ModuleState, Phase, Update};
pub use types::{
    Conversation, ConvoId, DeliveryState, GroupMember, Kind, Message, Status,
};
