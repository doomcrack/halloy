//! The real lp_* transport. Everything here runs on the dedicated logos
//! thread (per the crate-level threading contract), except the event
//! trampoline, which the protocol library invokes on its Asio IO thread.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::error::IpcError;
use crate::transport::{RawEvent, Transport};

const CORE_SERVICE: &CStr = c"core_service";
const MODULE_EVENT: &CStr = c"module_event";

/// Connection parameters for one lp client to `core_service`.
#[derive(Debug, Clone)]
pub struct FfiConfig {
    /// Named token issued by the daemon (`logoscore issue-token`), saved
    /// under the "core_service" key before the first call so the automatic
    /// capability mint path is skipped.
    pub token: String,
    /// Origin module name presented to the daemon (must match the
    /// issue-token name by convention).
    pub origin: String,
    pub core_service_port: u16,
    pub capability_port: u16,
}

/// Owned by the subscription for its lifetime; the trampoline forwards
/// events through it from the Asio IO thread.
struct Forwarder {
    events: UnboundedSender<RawEvent>,
}

/// Owns the `lp_client` + `lp_subscription` raw pointers. `connect()`:
/// `abi_ok()` → `lp_token_save("core_service", token)` →
/// `lp_client_create("core_service", origin, tcp_json(core), tcp_json(cap))`
/// → `lp_subscribe("module_event", trampoline, forwarder)` — subscription
/// armed BEFORE any invoke. The trampoline is `unsafe extern "C"`, wrapped
/// in `catch_unwind`, copies both C strings, and sends a [`RawEvent`] over
/// the unbounded channel — it never blocks and never panics across FFI.
/// `disconnect()`: `lp_unsubscribe` then `lp_client_destroy`, same thread,
/// this order. The forwarder box is intentionally leaked until disconnect
/// (the subscription may reference it until then).
pub struct FfiTransport {
    config: FfiConfig,
    client: *mut logos_sys::lp_client,
    subscription: *mut logos_sys::lp_subscription,
    forwarder: *mut Forwarder,
}

// The transport is moved onto the dedicated logos thread before `connect`,
// and every pointer is created, used, and destroyed on that thread only
// (the `Transport` contract) — moving the struct between threads while the
// pointers are null or unused from the old thread is sound.
unsafe impl Send for FfiTransport {}

impl FfiTransport {
    pub fn new(config: FfiConfig) -> Self {
        Self {
            config,
            client: std::ptr::null_mut(),
            subscription: std::ptr::null_mut(),
            forwarder: std::ptr::null_mut(),
        }
    }
}

impl Transport for FfiTransport {
    fn connect(
        &mut self,
        events: UnboundedSender<RawEvent>,
    ) -> Result<(), IpcError> {
        if !self.client.is_null() {
            return Err(IpcError::Create(
                "transport is already connected".to_owned(),
            ));
        }

        logos_sys::abi_ok().map_err(|major| {
            IpcError::Create(format!(
                "protocol ABI mismatch: library reports major {major}, expected {}",
                logos_sys::EXPECTED_ABI_MAJOR
            ))
        })?;

        let token = cstring(&self.config.token)?;
        let origin = cstring(&self.config.origin)?;
        let core_transport = cstring(&tcp_json(self.config.core_service_port))?;
        let capability_transport =
            cstring(&tcp_json(self.config.capability_port))?;

        let rc = unsafe {
            logos_sys::lp_token_save(CORE_SERVICE.as_ptr(), token.as_ptr())
        };
        if rc != logos_sys::LP_OK {
            return Err(IpcError::Create(format!(
                "lp_token_save failed with {rc}"
            )));
        }

        let client = unsafe {
            logos_sys::lp_client_create(
                CORE_SERVICE.as_ptr(),
                origin.as_ptr(),
                core_transport.as_ptr(),
                capability_transport.as_ptr(),
            )
        };
        if client.is_null() {
            return Err(IpcError::Create(
                "lp_client_create returned NULL".to_owned(),
            ));
        }

        let forwarder = Box::into_raw(Box::new(Forwarder { events }));
        let subscription = unsafe {
            logos_sys::lp_subscribe(
                client,
                MODULE_EVENT.as_ptr(),
                event_trampoline,
                forwarder.cast::<c_void>(),
            )
        };
        if subscription.is_null() {
            // No subscription was armed, so no callback can reference the
            // forwarder — safe to free it right away.
            unsafe {
                drop(Box::from_raw(forwarder));
                logos_sys::lp_client_destroy(client);
            }
            return Err(IpcError::Create(
                "lp_subscribe(module_event) returned NULL".to_owned(),
            ));
        }

        self.client = client;
        self.subscription = subscription;
        self.forwarder = forwarder;

        Ok(())
    }

    fn invoke(
        &mut self,
        method: &str,
        args_json: &str,
        timeout: Duration,
    ) -> Result<String, IpcError> {
        if self.client.is_null() {
            return Err(IpcError::Create("invoke before connect".to_owned()));
        }

        let method_c = cstring(method)?;
        let args_c = cstring(args_json)?;
        // `timeout_ms <= 0` selects the protocol default (20s) — clamp to
        // at least 1ms so tiny timeouts stay tiny.
        let timeout_ms = c_int::try_from(timeout.as_millis())
            .unwrap_or(c_int::MAX)
            .max(1);

        let mut out_result: *mut c_char = std::ptr::null_mut();
        let mut out_error: *mut c_char = std::ptr::null_mut();
        let rc = unsafe {
            logos_sys::lp_invoke(
                self.client,
                method_c.as_ptr(),
                args_c.as_ptr(),
                timeout_ms,
                &mut out_result,
                &mut out_error,
            )
        };
        let result = unsafe { logos_sys::take_string(out_result) };
        let error = unsafe { logos_sys::take_string(out_error) };

        if rc == logos_sys::LP_OK {
            return Ok(result.unwrap_or_else(|| "null".to_owned()));
        }
        Err(decode_error(rc, error))
    }

    fn disconnect(&mut self) {
        if !self.subscription.is_null() {
            unsafe { logos_sys::lp_unsubscribe(self.subscription) };
            self.subscription = std::ptr::null_mut();
        }
        if !self.forwarder.is_null() {
            // Safe to free only now: lp_unsubscribe guarantees no callback
            // fires after it returns.
            drop(unsafe { Box::from_raw(self.forwarder) });
            self.forwarder = std::ptr::null_mut();
        }
        if !self.client.is_null() {
            unsafe { logos_sys::lp_client_destroy(self.client) };
            self.client = std::ptr::null_mut();
        }
    }
}

impl Drop for FfiTransport {
    fn drop(&mut self) {
        // Safety net for teardown paths that skip an explicit disconnect;
        // idempotent because disconnect nulls every pointer.
        self.disconnect();
    }
}

/// Fires on the protocol library's Asio IO thread: copies both strings,
/// forwards them over the channel, and returns — never blocks, and never
/// lets a panic cross the FFI boundary.
unsafe extern "C" fn event_trampoline(
    event_name: *const c_char,
    data_json: *const c_char,
    user_data: *mut c_void,
) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| {
        if event_name.is_null() || data_json.is_null() || user_data.is_null() {
            return;
        }
        let name = unsafe { CStr::from_ptr(event_name) }
            .to_string_lossy()
            .into_owned();
        let payload_json = unsafe { CStr::from_ptr(data_json) }
            .to_string_lossy()
            .into_owned();
        let forwarder = unsafe { &*user_data.cast::<Forwarder>() };
        let _ = forwarder.events.send(RawEvent { name, payload_json });
    }));
}

fn tcp_json(port: u16) -> String {
    format!(
        r#"{{"protocol":"tcp","host":"127.0.0.1","port":{port},"codec":"json"}}"#
    )
}

fn cstring(s: &str) -> Result<CString, IpcError> {
    CString::new(s).map_err(|_| {
        IpcError::Create(
            "interior NUL in a string passed to the protocol".to_owned(),
        )
    })
}

/// Maps a failed `lp_invoke` to [`IpcError::Call`]: parses the canonical
/// `{"code","message","origin"}` object, falling back to the raw return
/// code when the error string is absent or unparsable.
fn decode_error(rc: c_int, error_json: Option<String>) -> IpcError {
    let Some(raw) = error_json else {
        return IpcError::Call {
            code: rc.to_string(),
            message: "lp_invoke failed without an error object".to_owned(),
            origin: String::new(),
        };
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return IpcError::Call {
            code: rc.to_string(),
            message: raw,
            origin: String::new(),
        };
    };
    let field = |key: &str| {
        value.get(key).map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
    };
    IpcError::Call {
        code: field("code").unwrap_or_else(|| rc.to_string()),
        message: field("message").unwrap_or_default(),
        origin: field("origin").unwrap_or_default(),
    }
}
