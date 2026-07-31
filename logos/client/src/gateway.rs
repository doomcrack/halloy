//! Typed core_service calls over the logos thread, including the
//! callModuleMethod envelope unwrap and the dead-link heuristic.
//!
//! Envelope (verified): `callModuleMethod(module, method, args)` returns
//! `{"status":"ok","module":...,"method":...,"result":<value>}` on success
//! or `{"status":"error","code":"MODULE_NOT_LOADED|METHOD_FAILED|
//! INVALID_ARGS|INTERNAL_ERROR","message":...}` on failure. Other gateway
//! methods return their value directly. An unknown method or arity returns
//! the literal `null`.
//!
//! Dead-link heuristic: every gateway method used here has a known non-null
//! result shape. Once `seen_ok` is set (first successful non-null result),
//! any later `LP_OK` + literal `null` ⇒ [`IpcError::DeadLink`] (upstream:
//! after the daemon dies, the cached plain-transport handle returns
//! LP_OK/"null" forever with no error). Before `seen_ok`, a `null` from a
//! known method maps to [`GatewayError::Unsupported`] instead.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::error::{GatewayError, IpcError, ModuleErrorCode};
use crate::thread::LogosHandle;

pub const STATUS_TIMEOUT: Duration = Duration::from_secs(3);
pub const LOAD_MODULE_TIMEOUT: Duration = Duration::from_secs(30);
pub const WATCH_TIMEOUT: Duration = Duration::from_secs(5);
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
/// Matches the protocol default so the outer guard, not the wire, decides.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone)]
pub struct Gateway {
    handle: Arc<LogosHandle>,
    seen_ok: Arc<AtomicBool>,
}

impl Gateway {
    pub fn new(handle: Arc<LogosHandle>) -> Self {
        Self {
            handle,
            seen_ok: Arc::new(AtomicBool::new(false)),
        }
    }

    /// `core_service.getStatus()` → the raw status object.
    pub async fn get_status(&self) -> Result<Value, GatewayError> {
        self.invoke_checked("getStatus", Vec::new(), STATUS_TIMEOUT)
            .await
    }

    /// `core_service.loadModule(name)`; dependencies auto-resolve.
    pub async fn load_module(&self, name: &str) -> Result<(), GatewayError> {
        let value = self
            .invoke_checked(
                "loadModule",
                vec![Value::String(name.to_owned())],
                LOAD_MODULE_TIMEOUT,
            )
            .await?;
        unwrap_envelope("loadModule", value)?;
        Ok(())
    }

    /// `core_service.watchModuleEvents(module, event)` → whether the watch
    /// was registered. Empty `event` requests all events of the module
    /// (wildcard — only use behind explicit opt-in, the QtRO leg is
    /// unverified).
    pub async fn watch_module_events(
        &self,
        module: &str,
        event: &str,
    ) -> Result<bool, GatewayError> {
        let value = self
            .invoke_checked(
                "watchModuleEvents",
                vec![
                    Value::String(module.to_owned()),
                    Value::String(event.to_owned()),
                ],
                WATCH_TIMEOUT,
            )
            .await?;
        value.as_bool().ok_or_else(|| {
            GatewayError::Decode(format!(
                "watchModuleEvents returned a non-boolean: {value}"
            ))
        })
    }

    /// `core_service.callModuleMethod(module, method, args)` with envelope
    /// unwrap: returns the inner `result` value.
    pub async fn call_module(
        &self,
        module: &str,
        method: &str,
        args: Vec<Value>,
        timeout: Duration,
    ) -> Result<Value, GatewayError> {
        let value = self
            .invoke_checked(
                "callModuleMethod",
                vec![
                    Value::String(module.to_owned()),
                    Value::String(method.to_owned()),
                    Value::Array(args),
                ],
                timeout,
            )
            .await?;
        unwrap_envelope(method, value)
    }

    /// `core_service.shutdown()`. The connection dropping mid-call is
    /// expected and tolerated: the daemon tears the link down while
    /// handling the call, so every transport-level outcome — an error
    /// object, a timeout, a closed channel, or the dead-link `null` — is
    /// mapped to success.
    pub async fn shutdown_daemon(&self) -> Result<(), GatewayError> {
        if let Err(error) = self
            .handle
            .invoke("shutdown", Value::Array(Vec::new()), SHUTDOWN_TIMEOUT)
            .await
        {
            log::debug!(
                "core_service.shutdown tore the link down (expected): {error}"
            );
        }
        Ok(())
    }

    /// One invoke with the dead-link heuristic applied (see the module
    /// docs): `null` maps to `DeadLink` after the first real result, to
    /// `Unsupported` before it; any non-null result arms `seen_ok`.
    async fn invoke_checked(
        &self,
        method: &str,
        args: Vec<Value>,
        timeout: Duration,
    ) -> Result<Value, GatewayError> {
        let value = self
            .handle
            .invoke(method, Value::Array(args), timeout)
            .await?;

        if value.is_null() {
            if self.seen_ok.load(Ordering::SeqCst) {
                return Err(GatewayError::Ipc(IpcError::DeadLink));
            }
            return Err(GatewayError::Unsupported {
                method: method.to_owned(),
            });
        }

        self.seen_ok.store(true, Ordering::SeqCst);
        Ok(value)
    }
}

/// Unwraps the `{"status":"ok"|"error",...}` envelope, returning the inner
/// `result` value (`null` when absent, as for `loadModule`).
fn unwrap_envelope(
    method: &str,
    mut value: Value,
) -> Result<Value, GatewayError> {
    match value.get("status").and_then(Value::as_str) {
        Some("ok") => {
            Ok(value.get_mut("result").map_or(Value::Null, Value::take))
        }
        Some("error") => {
            let code = ModuleErrorCode::parse(
                value.get("code").and_then(Value::as_str).unwrap_or(""),
            );
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            Err(GatewayError::Module { code, message })
        }
        _ => Err(GatewayError::Decode(format!(
            "{method}: response is not a status envelope: {value}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::fake::{FakeController, FakeTransport};

    async fn gateway() -> (Gateway, FakeController) {
        let (transport, controller) = FakeTransport::new();
        let (events_tx, _events_rx) = mpsc::unbounded_channel();
        let handle = LogosHandle::spawn(transport, events_tx).await.unwrap();
        (Gateway::new(Arc::new(handle)), controller)
    }

    #[tokio::test]
    async fn call_module_unwraps_ok_envelope() {
        let (gateway, controller) = gateway().await;
        controller.respond(
            "callModuleMethod",
            r#"{"status":"ok","module":"chat_module","method":"status","result":{"value":7}}"#,
        );

        let result = gateway
            .call_module(
                "chat_module",
                "status",
                vec![json!(1)],
                DEFAULT_CALL_TIMEOUT,
            )
            .await
            .unwrap();

        assert_eq!(result, json!({"value": 7}));
        assert_eq!(
            controller.calls(),
            vec![(
                "callModuleMethod".to_owned(),
                r#"["chat_module","status",[1]]"#.to_owned()
            )]
        );
    }

    #[tokio::test]
    async fn call_module_maps_every_error_code() {
        let (gateway, controller) = gateway().await;
        let codes = [
            ("MODULE_NOT_LOADED", ModuleErrorCode::ModuleNotLoaded),
            ("METHOD_FAILED", ModuleErrorCode::MethodFailed),
            ("INVALID_ARGS", ModuleErrorCode::InvalidArgs),
            ("INTERNAL_ERROR", ModuleErrorCode::Internal),
            ("SOMETHING_ELSE", ModuleErrorCode::Unknown),
        ];

        for (code, expected) in codes {
            controller.respond_once(
                "callModuleMethod",
                Ok(format!(
                    r#"{{"status":"error","code":"{code}","message":"boom"}}"#
                )),
            );
            let error = gateway
                .call_module(
                    "chat_module",
                    "status",
                    vec![],
                    DEFAULT_CALL_TIMEOUT,
                )
                .await
                .unwrap_err();
            assert!(
                matches!(
                    error,
                    GatewayError::Module { code, ref message }
                        if code == expected && message == "boom"
                ),
                "code {code} mapped to {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn load_module_accepts_ok_and_maps_error() {
        let (gateway, controller) = gateway().await;
        controller.respond_once(
            "loadModule",
            Ok(r#"{"status":"ok","module":"chat_module"}"#.to_owned()),
        );
        gateway.load_module("chat_module").await.unwrap();

        controller.respond_once(
            "loadModule",
            Ok(
                r#"{"status":"error","code":"INTERNAL_ERROR","message":"no"}"#
                    .to_owned(),
            ),
        );
        let error = gateway.load_module("chat_module").await.unwrap_err();
        assert!(matches!(
            error,
            GatewayError::Module {
                code: ModuleErrorCode::Internal,
                ..
            }
        ));

        assert_eq!(
            controller.calls(),
            vec![
                ("loadModule".to_owned(), r#"["chat_module"]"#.to_owned()),
                ("loadModule".to_owned(), r#"["chat_module"]"#.to_owned()),
            ]
        );
    }

    #[tokio::test]
    async fn watch_module_events_returns_the_bool() {
        let (gateway, controller) = gateway().await;
        controller.respond("watchModuleEvents", "true");

        let watched = gateway
            .watch_module_events("chat_module", "message_received")
            .await
            .unwrap();

        assert!(watched);
        assert_eq!(
            controller.calls(),
            vec![(
                "watchModuleEvents".to_owned(),
                r#"["chat_module","message_received"]"#.to_owned()
            )]
        );
    }

    #[tokio::test]
    async fn null_after_a_success_is_a_dead_link() {
        let (gateway, controller) = gateway().await;
        controller
            .respond_once("getStatus", Ok(r#"{"modules":[]}"#.to_owned()));

        gateway.get_status().await.unwrap();
        let error = gateway.get_status().await.unwrap_err();

        assert!(matches!(error, GatewayError::Ipc(IpcError::DeadLink)));
    }

    #[tokio::test]
    async fn null_before_any_success_is_unsupported() {
        let (gateway, _controller) = gateway().await;

        let error = gateway.get_status().await.unwrap_err();

        assert!(matches!(
            error,
            GatewayError::Unsupported { method } if method == "getStatus"
        ));
    }

    #[tokio::test]
    async fn shutdown_daemon_tolerates_transport_errors() {
        let (gateway, controller) = gateway().await;
        controller.fail_all(IpcError::Call {
            code: "-4".to_owned(),
            message: "connection closed".to_owned(),
            origin: "transport".to_owned(),
        });

        gateway.shutdown_daemon().await.unwrap();
    }
}
