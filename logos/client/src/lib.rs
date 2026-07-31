//! Qt-free client for a logoscore daemon over the `lp_*` C ABI.
//!
//! Architecture (all constraints verified against logos-protocol source):
//! - ONE dedicated OS thread ([`thread::LogosHandle`]) owns the `lp_client`:
//!   create, every invoke, and destroy all happen on it. Cross-thread lp
//!   calls deadlock in a Qt-free process.
//! - Synchronous `lp_invoke` only; the async variant silently drops
//!   completions without a Qt event loop.
//! - `lp_subscribe("module_event", ...)` is armed inside
//!   [`Transport::connect`] BEFORE any other call, so no relayed event is
//!   lost to the subscribe-after-enable race. Event callbacks fire on the
//!   library's Asio IO thread: the trampoline only copies strings into an
//!   unbounded channel.
//! - Dead-link heuristic ([`gateway::Gateway`]): after the first successful
//!   call, `LP_OK` with a literal `null` result means the connection died
//!   (upstream: cached handles never invalidate) → surface
//!   [`error::IpcError::DeadLink`].

pub mod error;
pub mod event;
pub mod fake;
#[cfg(feature = "ffi")]
pub mod ffi_transport;
pub mod gateway;
pub mod thread;
pub mod transport;

pub use error::{GatewayError, IpcError, ModuleErrorCode};
pub use event::{ModuleEvent, decode_module_event};
pub use fake::FakeTransport;
#[cfg(feature = "ffi")]
pub use ffi_transport::FfiTransport;
pub use gateway::Gateway;
pub use thread::LogosHandle;
pub use transport::{RawEvent, Transport};
