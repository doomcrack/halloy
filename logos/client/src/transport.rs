//! The seam between the logos thread and the wire. `FfiTransport` is the
//! real lp_* implementation; `FakeTransport` drives the whole stack in unit
//! tests without the dylib or a daemon.

use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::error::IpcError;

/// An event as delivered by the wire, copied verbatim off the IO thread.
/// `payload_json` is the JSON array the protocol delivered; parsing happens
/// on the consumer side, never on the IO thread.
#[derive(Debug, Clone)]
pub struct RawEvent {
    pub name: String,
    pub payload_json: String,
}

/// Blocking transport driven exclusively from the dedicated logos thread.
///
/// Contract:
/// - `connect` establishes the client AND arms the `module_event`
///   subscription before returning; forwarded events flow into `events`
///   from arbitrary threads without blocking.
/// - `invoke` blocks the calling (logos) thread until the result or
///   `timeout`; `args_json` is a JSON array; the returned string is the
///   result JSON value.
/// - `disconnect` tears down subscription then client, on the same thread
///   that called `connect`.
pub trait Transport: Send + 'static {
    fn connect(
        &mut self,
        events: UnboundedSender<RawEvent>,
    ) -> Result<(), IpcError>;

    fn invoke(
        &mut self,
        method: &str,
        args_json: &str,
        timeout: Duration,
    ) -> Result<String, IpcError>;

    fn disconnect(&mut self);
}
