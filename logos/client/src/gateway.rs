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
/// Longer than [`STATUS_TIMEOUT`] because `getModuleInfo` is the one
/// introspection call that leaves the daemon: for a loaded module it
/// forwards `getPluginMethods` and `getPluginEvents` to the module itself,
/// so it inherits that module's dispatch latency.
pub const MODULE_INFO_TIMEOUT: Duration = Duration::from_secs(5);

/// One `listModules` row — the daemon's own view of one installed module.
///
/// `status` stays a verbatim string rather than an enum: this is a wire
/// boundary, and the vocabulary is the daemon's to grow. Today it only ever
/// emits `loaded` / `not_loaded` (`crashed` is counted by `getStatus` but
/// never produced by the row builder), and mapping those onto a domain state
/// belongs to the layer that owns the domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleStatus {
    pub name: String,
    pub status: String,
    /// From the module's manifest. The daemon substitutes an empty string
    /// when the manifest carries no version; that is an absence, not a
    /// version, so it is normalised to `None`.
    pub version: Option<String>,
}

/// `getModuleInfo`, reduced to what a monitor reads.
///
/// `methods` and `events` are the *live* contract, introspected from the
/// running module rather than read from a manifest — which is why they are
/// empty for a module that is not loaded, and why an event-watch list must
/// be built from them instead of being written down. The same blockchain
/// module ships with one event on the tag we run and three on master.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInfo {
    pub name: String,
    pub status: String,
    pub version: Option<String>,
    pub dependencies: Vec<String>,
    pub dependents: Vec<String>,
    pub methods: Vec<String>,
    pub events: Vec<String>,
}

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

    /// `core_service.listModules(filter)` → one row per installed module.
    ///
    /// Always asks for `"all"`, never `"loaded"`: a monitor that only saw
    /// loaded modules could not tell "staged but idle" from "gone", and the
    /// idle case is the normal one for everything except chat.
    ///
    /// Unlike `callModuleMethod` this returns a bare JSON array, not a
    /// status envelope — the daemon builds the rows itself and never
    /// consults the modules, so there is nothing for a module to fail. A row
    /// that does not decode is dropped with a warning rather than failing
    /// the whole poll; one malformed entry must not blank the sidebar.
    pub async fn list_modules(
        &self,
    ) -> Result<Vec<ModuleStatus>, GatewayError> {
        let value = self
            .invoke_checked(
                "listModules",
                vec![Value::String("all".to_owned())],
                STATUS_TIMEOUT,
            )
            .await?;

        let Value::Array(rows) = value else {
            return Err(GatewayError::Decode(format!(
                "listModules returned a non-array: {value}"
            )));
        };

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let Some(name) = row.get("name").and_then(Value::as_str) else {
                    log::warn!("listModules row without a name: {row}");
                    return None;
                };
                Some(ModuleStatus {
                    name: name.to_owned(),
                    status: row
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    version: non_empty(row.get("version")),
                })
            })
            .collect())
    }

    /// `core_service.getModuleInfo(name)` → the module's dependency edges
    /// and its live method/event contract.
    ///
    /// Two shapes share one call. Success is a bare map whose `status` is
    /// the module's *load* state (`loaded` / `not_loaded`); failure is the
    /// error half of the usual envelope (`status: "error"` with
    /// `MODULE_NOT_FOUND`). So this cannot go through
    /// [`unwrap_envelope`] — that would read a perfectly good answer as a
    /// malformed one.
    ///
    /// Treat every failure here as "this signal is unavailable", never as a
    /// reason to tear a session down. A daemon predating the method returns
    /// the literal `null`, which the dead-link heuristic reads as
    /// [`IpcError::DeadLink`] once the link has produced any earlier
    /// result — indistinguishable, at this layer, from a daemon that died.
    pub async fn module_info(
        &self,
        name: &str,
    ) -> Result<ModuleInfo, GatewayError> {
        let value = self
            .invoke_checked(
                "getModuleInfo",
                vec![Value::String(name.to_owned())],
                MODULE_INFO_TIMEOUT,
            )
            .await?;

        let status = value
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                GatewayError::Decode(format!(
                    "getModuleInfo returned no status: {value}"
                ))
            })?
            .to_owned();

        if status == "error" {
            return Err(GatewayError::Module {
                code: ModuleErrorCode::parse(
                    value.get("code").and_then(Value::as_str).unwrap_or(""),
                ),
                message: value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            });
        }

        Ok(ModuleInfo {
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_owned(),
            status,
            version: non_empty(value.get("version")),
            dependencies: names(value.get("dependencies")),
            dependents: names(value.get("dependents")),
            methods: names(value.get("methods")),
            events: names(value.get("events")),
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

/// A string field the daemon fills with `""` when it has nothing to say
/// (module versions, notably) read as the absence it is.
fn non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Flattens the two shapes `getModuleInfo` mixes in one response: plain
/// strings (`dependencies`, `dependents`) and interface entries
/// (`methods`, `events`), which the module's proxy emits as objects
/// `{name, type, signature, returnType, parameters}`. Anything else in the
/// array is skipped rather than guessed at.
fn names(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| match item {
            Value::String(name) => Some(name.clone()),
            other => {
                other.get("name").and_then(Value::as_str).map(str::to_owned)
            }
        })
        .collect()
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

    /// The row builder is the daemon's, not a module's: it fills `version`
    /// with `""` when the manifest has none, and that absence must survive as
    /// `None` rather than becoming an empty-string "version".
    ///
    /// The `uptime_seconds` the daemon also sends is deliberately not decoded —
    /// it advances every second, and folding it into a state the monitor
    /// compares made every poll look like a change. The fixture keeps it to
    /// pin that an unread wire field is ignored rather than breaking the parse.
    #[tokio::test]
    async fn list_modules_decodes_rows_and_asks_for_all() {
        let (gateway, controller) = gateway().await;
        controller.respond(
            "listModules",
            r#"[{"name":"chat_module","status":"loaded","version":"0.2.1","uptime_seconds":42},
                {"name":"blockchain_module","status":"not_loaded","version":""}]"#,
        );

        let modules = gateway.list_modules().await.unwrap();

        assert_eq!(
            modules,
            vec![
                ModuleStatus {
                    name: "chat_module".to_owned(),
                    status: "loaded".to_owned(),
                    version: Some("0.2.1".to_owned()),
                },
                ModuleStatus {
                    name: "blockchain_module".to_owned(),
                    status: "not_loaded".to_owned(),
                    version: None,
                },
            ]
        );
        assert_eq!(
            controller.calls(),
            vec![("listModules".to_owned(), r#"["all"]"#.to_owned())]
        );
    }

    /// One unusable row must not cost the caller every other row — a poll
    /// that returns nothing blanks the whole module sidebar.
    #[tokio::test]
    async fn list_modules_drops_only_the_undecodable_row() {
        let (gateway, controller) = gateway().await;
        controller.respond(
            "listModules",
            r#"[{"status":"loaded"},{"name":"delivery_module","status":"loaded"}]"#,
        );

        let modules = gateway.list_modules().await.unwrap();

        assert_eq!(
            modules.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            vec!["delivery_module"]
        );
    }

    /// `getModuleInfo` answers with a bare map whose `status` is the load
    /// state, so it must NOT be read as an ok/error envelope. Events arrive
    /// as interface objects; dependencies as plain strings.
    #[tokio::test]
    async fn module_info_decodes_both_list_shapes() {
        let (gateway, controller) = gateway().await;
        controller.respond(
            "getModuleInfo",
            r#"{"name":"chat_module","status":"loaded","version":"0.2.1",
                "dependencies":["delivery_module"],"dependents":[],
                "methods":[{"name":"status","type":"method"}],
                "events":[{"name":"message_received","type":"event"},
                          {"name":"delivery_state_changed","type":"event"}]}"#,
        );

        let info = gateway.module_info("chat_module").await.unwrap();

        assert_eq!(
            info,
            ModuleInfo {
                name: "chat_module".to_owned(),
                status: "loaded".to_owned(),
                version: Some("0.2.1".to_owned()),
                dependencies: vec!["delivery_module".to_owned()],
                dependents: Vec::new(),
                methods: vec!["status".to_owned()],
                events: vec![
                    "message_received".to_owned(),
                    "delivery_state_changed".to_owned(),
                ],
            }
        );
        assert_eq!(
            controller.calls(),
            vec![("getModuleInfo".to_owned(), r#"["chat_module"]"#.to_owned())]
        );
    }

    /// An idle module answers with no `methods`/`events` at all — the
    /// daemon only introspects a running one. Empty lists, not an error.
    #[tokio::test]
    async fn module_info_of_an_idle_module_has_no_contract() {
        let (gateway, controller) = gateway().await;
        controller.respond(
            "getModuleInfo",
            r#"{"name":"blockchain_module","status":"not_loaded","version":"0.2.0",
                "dependencies":[],"dependents":[]}"#,
        );

        let info = gateway.module_info("blockchain_module").await.unwrap();

        assert_eq!(info.status, "not_loaded");
        assert!(info.methods.is_empty() && info.events.is_empty());
    }

    #[tokio::test]
    async fn module_info_maps_the_not_found_error() {
        let (gateway, controller) = gateway().await;
        controller.respond(
            "getModuleInfo",
            r#"{"status":"error","code":"MODULE_NOT_FOUND","message":"Module 'nope' not found."}"#,
        );

        let error = gateway.module_info("nope").await.unwrap_err();

        assert!(matches!(
            error,
            GatewayError::Module {
                code: ModuleErrorCode::ModuleNotFound,
                ..
            }
        ));
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
