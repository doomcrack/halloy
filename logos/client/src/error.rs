//! Error taxonomy, layered: `IpcError` (transport/ABI) → `GatewayError`
//! (core_service envelope) → chat-level errors live in the logos-chat crate.

#[derive(Debug, Clone, thiserror::Error)]
pub enum IpcError {
    #[error("failed to create lp client: {0}")]
    Create(String),
    /// Canonical protocol error object `{"code","message","origin"}`.
    #[error("call failed: {code}: {message} (origin {origin})")]
    Call {
        code: String,
        message: String,
        origin: String,
    },
    #[error("call to {method} timed out")]
    Timeout { method: String },
    /// `LP_OK` with a literal `null` result after the link had already
    /// produced real results — the daemon-side connection is gone and the
    /// cached handle will never recover (verified upstream behavior).
    #[error("connection to the daemon is dead")]
    DeadLink,
    #[error("could not decode response: {0}")]
    Decode(String),
    #[error("logos thread is gone")]
    ChannelClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleErrorCode {
    ModuleNotLoaded,
    /// Only `getModuleInfo` produces this one, and it means the name is not
    /// installed at all — distinct from `ModuleNotLoaded`, which means it is
    /// installed and idle.
    ModuleNotFound,
    MethodFailed,
    InvalidArgs,
    Internal,
    Unknown,
}

impl ModuleErrorCode {
    pub fn parse(code: &str) -> Self {
        match code {
            "MODULE_NOT_LOADED" => Self::ModuleNotLoaded,
            "MODULE_NOT_FOUND" => Self::ModuleNotFound,
            "METHOD_FAILED" => Self::MethodFailed,
            "INVALID_ARGS" => Self::InvalidArgs,
            "INTERNAL_ERROR" => Self::Internal,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum GatewayError {
    #[error(transparent)]
    Ipc(#[from] IpcError),
    /// core_service reported `{status:"error", code, message}`.
    #[error("module error {code:?}: {message}")]
    Module {
        code: ModuleErrorCode,
        message: String,
    },
    /// A known gateway method returned `null` before the link had ever
    /// produced a real result — the method is missing on this daemon.
    #[error("core_service does not support {method}")]
    Unsupported { method: String },
    #[error("could not decode gateway response: {0}")]
    Decode(String),
}
