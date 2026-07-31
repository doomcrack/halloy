//! Raw bindings to the logos-protocol `lp_*` C ABI (`logos_protocol.h`,
//! protocol version 0.2.0). JSON-in-strings data model: method arguments are
//! a JSON array, results a JSON value, event payloads a JSON array — all
//! UTF-8 `const char*`.
//!
//! Ownership contract (from the header): every `char*` RETURNED by the
//! library is heap-allocated and must be freed with [`lp_string_free`]
//! (safe on NULL); every `const char*` passed in is borrowed for the call.
//!
//! Threading contract that OVERRIDES the header's claims for Qt-free
//! processes (verified against the implementation):
//! - One dedicated thread must create, invoke on, and destroy each
//!   `lp_client`. Cross-thread calls marshal via a Qt blocking queued
//!   connection that never dispatches without a Qt event loop → deadlock.
//! - `lp_invoke_async` completions are posted through
//!   `QCoreApplication::instance()` and are SILENTLY DROPPED when absent.
//!   Qt-free consumers must use synchronous [`lp_invoke`] only.
//! - `lp_subscribe` event callbacks fire inline on the library's single
//!   Asio IO worker thread; they must copy the strings and return
//!   immediately, never block, and never panic across the FFI boundary.

#![allow(non_camel_case_types)]

use std::ffi::{CStr, c_char, c_int, c_void};

pub const LP_OK: c_int = 0;
pub const LP_ERR_INVALID_ARG: c_int = -1;
pub const LP_ERR_UNSUPPORTED: c_int = -2;
pub const LP_ERR_INTERNAL: c_int = -3;
pub const LP_ERR_UNAVAILABLE: c_int = -4;

/// The protocol MAJOR this crate was written against. Interoperation is
/// guaranteed iff the linked library reports the same MAJOR.
pub const EXPECTED_ABI_MAJOR: c_int = 0;

#[repr(C)]
pub struct lp_client {
    _private: [u8; 0],
}

#[repr(C)]
pub struct lp_subscription {
    _private: [u8; 0],
}

/// Result callback for [`lp_invoke_async`]. `ok != 0` → `json` is the result
/// JSON value; `ok == 0` → `json` is the canonical error object
/// `{"code","message","origin"}`. `json` is only valid for the duration of
/// the callback.
pub type lp_result_cb = unsafe extern "C" fn(
    ok: c_int,
    json: *const c_char,
    user_data: *mut c_void,
);

/// Event callback for [`lp_subscribe`]. `data_json` is a JSON array (the
/// event payload), valid only for the duration of the callback.
pub type lp_event_cb = unsafe extern "C" fn(
    event_name: *const c_char,
    data_json: *const c_char,
    user_data: *mut c_void,
);

unsafe extern "C" {
    /// Static string — do NOT free.
    pub fn lp_protocol_version() -> *const c_char;
    pub fn lp_protocol_abi_major() -> c_int;

    /// Free a string returned by this library. Safe to call with NULL.
    pub fn lp_string_free(s: *mut c_char);

    /// "remote" (IPC, default) | "local" | "mock".
    pub fn lp_set_mode(mode: *const c_char) -> c_int;
    /// Static string — do not free.
    pub fn lp_get_mode() -> *const c_char;
    pub fn lp_set_default_transport(transport_json: *const c_char) -> c_int;

    /// Create a client for calling `target_module` on behalf of
    /// `origin_module`. Both transport JSONs must be explicit plain-TCP
    /// objects in a Qt-free process — NULL falls back to the process default
    /// (QtRO LocalSocket), which stalls and misbehaves without a Qt loop.
    /// The calling thread becomes the client's owner thread.
    pub fn lp_client_create(
        target_module: *const c_char,
        origin_module: *const c_char,
        target_transport_json: *const c_char,
        capability_transport_json: *const c_char,
    ) -> *mut lp_client;

    /// After this returns, no further callbacks fire for the client or its
    /// subscriptions. Must be called on the owner thread in a Qt-free
    /// process (a foreign-thread destroy defers to `deleteLater`, which
    /// never runs without a Qt loop and leaks).
    pub fn lp_client_destroy(client: *mut lp_client);

    /// Blocking call. `args_json` is a JSON array (NULL means "[]");
    /// `timeout_ms <= 0` selects the default (currently 20s). On LP_OK,
    /// `*out_result_json` receives the result JSON value (may be the literal
    /// "null" — indistinguishable from a dead cached connection, see the
    /// dead-link heuristic in logos-client). On failure `*out_error_json`
    /// receives the canonical error object. Both out-strings are owned by
    /// the caller.
    pub fn lp_invoke(
        client: *mut lp_client,
        method: *const c_char,
        args_json: *const c_char,
        timeout_ms: c_int,
        out_result_json: *mut *mut c_char,
        out_error_json: *mut *mut c_char,
    ) -> c_int;

    /// BROKEN in Qt-free processes (completion silently dropped). Bound for
    /// completeness; do not use outside a Qt host.
    pub fn lp_invoke_async(
        client: *mut lp_client,
        method: *const c_char,
        args_json: *const c_char,
        timeout_ms: c_int,
        cb: lp_result_cb,
        user_data: *mut c_void,
    ) -> c_int;

    /// Subscribe to `event_name` on the client's target. Returns NULL on
    /// failure. NOTE: a second subscription for the same (object, event)
    /// silently overwrites the first server-side; subscribe exactly once.
    pub fn lp_subscribe(
        client: *mut lp_client,
        event_name: *const c_char,
        cb: lp_event_cb,
        user_data: *mut c_void,
    ) -> *mut lp_subscription;

    /// Silences the callback. Known upstream gap: no Unsubscribe frame is
    /// sent and the underlying object is not released.
    pub fn lp_unsubscribe(sub: *mut lp_subscription);

    /// JSON array of the target's methods/events. Caller frees. NULL on
    /// failure.
    pub fn lp_get_methods(client: *mut lp_client) -> *mut c_char;

    /// Stored token for `module_name`; NULL when absent. Caller frees.
    pub fn lp_token_get(module_name: *const c_char) -> *mut c_char;
    pub fn lp_token_save(
        module_name: *const c_char,
        token: *const c_char,
    ) -> c_int;
    pub fn lp_inform_module_token(
        client: *mut lp_client,
        auth_token: *const c_char,
        module_name: *const c_char,
        token: *const c_char,
    ) -> c_int;
}

/// Checks that the linked library speaks the MAJOR this crate targets.
/// Call once at startup before any other lp_* use.
pub fn abi_ok() -> Result<(), i32> {
    let got = unsafe { lp_protocol_abi_major() };
    if got == EXPECTED_ABI_MAJOR {
        Ok(())
    } else {
        Err(got)
    }
}

/// Version string of the linked library (static storage — no free needed).
pub fn version() -> String {
    let ptr = unsafe { lp_protocol_version() };
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Copies a library-owned heap string into a `String` and frees the
/// original. Returns `None` for NULL.
///
/// # Safety
/// `p` must be NULL or a pointer returned by this library that has not
/// already been freed.
pub unsafe fn take_string(p: *mut c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { lp_string_free(p) };
    Some(s)
}
