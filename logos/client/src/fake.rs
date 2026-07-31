//! Scripted in-memory transport: drives the full client + session stack in
//! unit tests (and the UI's mock mode) without the dylib or a daemon.
//!
//! Response resolution per invoke, in order:
//! 1. a queued one-shot response for the method ([`FakeController::respond_once`]),
//! 2. the method's standing responder ([`FakeController::respond_with`] /
//!    [`FakeController::respond`]),
//! 3. a standing global failure ([`FakeController::fail_all`]),
//! 4. otherwise the literal `"null"` — mimicking the daemon's
//!    unknown-method behavior (and, after first success, the dead-link
//!    signature).
//!
//! Every invoke is appended to an ordered call log regardless of outcome.
//! Events injected via [`FakeController::emit`] are delivered through the
//! same channel `Transport::connect` was given, exactly like wire events.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::error::IpcError;
use crate::transport::{RawEvent, Transport};

type Responder = Box<dyn Fn(&str) -> Result<String, IpcError> + Send + Sync>;

#[derive(Default)]
struct Shared {
    responders: HashMap<String, Arc<Responder>>,
    one_shots: HashMap<String, VecDeque<Result<String, IpcError>>>,
    failure: Option<IpcError>,
    calls: Vec<(String, String)>,
    events: Option<UnboundedSender<RawEvent>>,
    connected: bool,
}

/// The transport half: hand to `LogosHandle::spawn` / `session::run`.
pub struct FakeTransport(Arc<Mutex<Shared>>);

/// The test/mock half: script responses, inject events, inspect calls.
/// Cloneable and Send so tests can drive it while the session runs.
#[derive(Clone)]
pub struct FakeController(Arc<Mutex<Shared>>);

impl FakeTransport {
    pub fn new() -> (Self, FakeController) {
        let shared = Arc::new(Mutex::new(Shared::default()));
        (Self(shared.clone()), FakeController(shared))
    }
}

impl FakeController {
    /// Standing responder for `method`, evaluated per call with the
    /// args JSON. Called without the internal lock held, so it may sleep
    /// or call back into this controller (e.g. `emit`).
    pub fn respond_with(
        &self,
        method: &str,
        f: impl Fn(&str) -> Result<String, IpcError> + Send + Sync + 'static,
    ) {
        let mut shared = self.0.lock().unwrap();
        shared
            .responders
            .insert(method.to_owned(), Arc::new(Box::new(f)));
    }

    /// Standing fixed JSON result for every call of `method`.
    pub fn respond(&self, method: &str, result_json: &str) {
        let result_json = result_json.to_owned();
        self.respond_with(method, move |_| Ok(result_json.clone()));
    }

    /// Queue a response consumed by exactly one call of `method` (FIFO),
    /// taking precedence over the standing responder.
    pub fn respond_once(&self, method: &str, result: Result<String, IpcError>) {
        let mut shared = self.0.lock().unwrap();
        shared
            .one_shots
            .entry(method.to_owned())
            .or_default()
            .push_back(result);
    }

    /// Deliver an event as if it arrived from the wire. Panics if called
    /// before the transport connected.
    pub fn emit(&self, name: &str, payload_json: &str) {
        let shared = self.0.lock().unwrap();
        let events = shared
            .events
            .as_ref()
            .expect("emit before the transport connected");
        let _ = events.send(RawEvent {
            name: name.to_owned(),
            payload_json: payload_json.to_owned(),
        });
    }

    /// Ordered `(method, args_json)` log of every invoke so far.
    pub fn calls(&self) -> Vec<(String, String)> {
        self.0.lock().unwrap().calls.clone()
    }

    /// Whether `Transport::connect` has run (i.e. the subscription is
    /// armed).
    pub fn connected(&self) -> bool {
        self.0.lock().unwrap().connected
    }

    /// Make every subsequent invoke fail with `error` until cleared by a
    /// standing/one-shot responder.
    pub fn fail_all(&self, error: IpcError) {
        self.0.lock().unwrap().failure = Some(error);
    }
}

impl Transport for FakeTransport {
    fn connect(
        &mut self,
        events: UnboundedSender<RawEvent>,
    ) -> Result<(), IpcError> {
        let mut shared = self.0.lock().unwrap();
        shared.events = Some(events);
        shared.connected = true;
        Ok(())
    }

    fn invoke(
        &mut self,
        method: &str,
        args_json: &str,
        _timeout: Duration,
    ) -> Result<String, IpcError> {
        let responder = {
            let mut shared = self.0.lock().unwrap();
            shared.calls.push((method.to_owned(), args_json.to_owned()));

            if let Some(queue) = shared.one_shots.get_mut(method)
                && let Some(result) = queue.pop_front()
            {
                return result;
            }

            match shared.responders.get(method) {
                Some(responder) => Arc::clone(responder),
                None => {
                    return match &shared.failure {
                        Some(error) => Err(error.clone()),
                        None => Ok("null".to_owned()),
                    };
                }
            }
        };

        responder(args_json)
    }

    fn disconnect(&mut self) {
        let mut shared = self.0.lock().unwrap();
        shared.events = None;
        shared.connected = false;
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(1);

    #[test]
    fn resolves_once_then_standing_then_failure_then_null() {
        let (mut transport, controller) = FakeTransport::new();
        let (events_tx, _events_rx) = mpsc::unbounded_channel();
        transport.connect(events_tx).unwrap();

        assert_eq!(transport.invoke("m", "[]", TIMEOUT).unwrap(), "null");

        controller.respond("m", "1");
        controller.respond_once("m", Ok("2".to_owned()));
        assert_eq!(transport.invoke("m", "[]", TIMEOUT).unwrap(), "2");
        assert_eq!(transport.invoke("m", "[]", TIMEOUT).unwrap(), "1");

        controller.fail_all(IpcError::DeadLink);
        assert_eq!(transport.invoke("m", "[]", TIMEOUT).unwrap(), "1");
        assert!(matches!(
            transport.invoke("other", "[]", TIMEOUT),
            Err(IpcError::DeadLink)
        ));

        assert_eq!(
            controller
                .calls()
                .into_iter()
                .map(|(method, args)| format!("{method}{args}"))
                .collect::<Vec<_>>(),
            vec!["m[]", "m[]", "m[]", "m[]", "other[]"]
        );
    }

    #[test]
    fn tracks_connection_state_and_delivers_events() {
        let (mut transport, controller) = FakeTransport::new();
        assert!(!controller.connected());

        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        transport.connect(events_tx).unwrap();
        assert!(controller.connected());

        controller.emit("module_event", r#"["chat_module","status_changed"]"#);
        let event = events_rx.try_recv().unwrap();
        assert_eq!(event.name, "module_event");
        assert_eq!(event.payload_json, r#"["chat_module","status_changed"]"#);

        transport.disconnect();
        assert!(!controller.connected());
    }

    #[test]
    #[should_panic(expected = "emit before the transport connected")]
    fn emit_before_connect_panics() {
        let (_transport, controller) = FakeTransport::new();
        controller.emit("module_event", "[]");
    }
}
