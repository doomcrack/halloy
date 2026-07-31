//! Chat-level and session-level error taxonomy. `BackendError` is fatal for
//! a session run (surfaced as `Update::Fatal`); `ActionError` is per-action
//! and non-fatal (surfaced as `Update::ActionFailed`).

use std::path::PathBuf;

use logos_client::GatewayError;

use crate::types::ConvoId;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ChatError {
    #[error(transparent)]
    Gateway(#[from] GatewayError),
    /// The module-level `result` envelope reported `success == false`.
    #[error("{0}")]
    Unsuccessful(String),
    #[error("could not decode chat_module response: {0}")]
    Decode(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadKind {
    Messages,
    Members,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum BackendError {
    #[error("missing {what}; searched {searched:?}")]
    ArtifactsMissing {
        what: String,
        searched: Vec<PathBuf>,
    },
    #[error("daemon failed to start: {0}")]
    DaemonStartFailed(String),
    #[error("daemon unhealthy: {0}")]
    DaemonUnhealthy(String),
    #[error("token issuance failed: {0}")]
    TokenFailed(String),
    #[error("token rejected by daemon: {0}")]
    TokenRejected(String),
    #[error(
        "logos-protocol ABI mismatch: expected major {expected}, linked {got}"
    )]
    AbiMismatch { expected: i32, got: i32 },
    #[error("could not create protocol client: {0}")]
    ClientCreateFailed(String),
    #[error("chat_module failed to load: {0}")]
    ModuleLoadFailed(String),
    #[error("could not watch {event}: {detail}")]
    WatchFailed { event: String, detail: String },
    #[error("chat init failed: {0}")]
    ChatInitFailed(String),
    #[error("gave up restarting the backend after {attempts} attempts: {last}")]
    RestartExhausted { attempts: u32, last: String },
    #[error("this build has no live transport (ffi feature disabled)")]
    FfiUnavailable,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ActionError {
    #[error("chat is not online")]
    NotOnline { attempted: &'static str },
    #[error("invalid {field}")]
    InvalidInput { field: &'static str },
    /// Carries the composed text so the UI can restore the draft.
    #[error("failed to send: {reason}")]
    SendFailed {
        convo_id: ConvoId,
        content: String,
        reason: String,
    },
    #[error("{method} failed: {reason}")]
    Module {
        method: &'static str,
        reason: String,
    },
    #[error("failed to load {what:?}: {reason}")]
    LoadFailed {
        convo_id: ConvoId,
        what: LoadKind,
        reason: String,
    },
    #[error("{method} timed out")]
    Timeout { method: &'static str },
}
