//! The dedicated logos thread: sole owner of the transport (and, through
//! it, the `lp_client`). Commands arrive over a blocking channel; replies
//! leave via tokio oneshots (safe to complete from a non-async thread).
//!
//! Guarantees:
//! - Strict serialization: one command at a time — overlapping `lp_invoke`s
//!   to the single-dispatch chat module are impossible by construction.
//! - The outer await wraps the oneshot in `tokio::time::timeout(timeout +
//!   GRACE)` so a wedged native call cannot hang the session forever;
//!   tripping it surfaces as [`IpcError::Timeout`] and is treated upstream
//!   as dead-link suspicion.
//! - `shutdown` runs `Transport::disconnect` on the logos thread and joins
//!   with a deadline (a worst-case in-flight invoke holds the thread for its
//!   full timeout); past the deadline the thread is detached — process exit
//!   reclaims it. The join runs on a plain std thread, never on tokio's
//!   blocking pool: a timed-out `spawn_blocking` would keep occupying a
//!   pool thread and stall runtime shutdown until the logos thread exits.

use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::error::IpcError;
use crate::transport::{RawEvent, Transport};

/// Extra slack on top of the caller-requested timeout before the awaiting
/// side gives up on the logos thread.
pub const OUTER_TIMEOUT_GRACE: Duration = Duration::from_secs(2);

/// Deadline for joining the logos thread at shutdown (must exceed the
/// default 20s protocol timeout an in-flight invoke may be blocked on).
pub const JOIN_DEADLINE: Duration = Duration::from_secs(25);

enum Cmd {
    Invoke {
        method: String,
        args: Value,
        timeout: Duration,
        reply: oneshot::Sender<Result<Value, IpcError>>,
    },
    Shutdown,
}

/// Handle to the logos thread. Cheap to clone via `Arc` at the call sites
/// that need sharing; the handle itself owns the join handle.
pub struct LogosHandle {
    cmd_tx: std::sync::mpsc::Sender<Cmd>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl LogosHandle {
    /// Spawns the logos thread and runs `Transport::connect` on it (which
    /// arms the `module_event` subscription before returning). The returned
    /// future resolves once connected, or with the connect error. Events
    /// flow into `events` for the lifetime of the connection.
    pub async fn spawn<T: Transport>(
        mut transport: T,
        events: UnboundedSender<RawEvent>,
    ) -> Result<Self, IpcError> {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Cmd>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), IpcError>>();

        let join = std::thread::Builder::new()
            .name("logos".to_owned())
            .spawn(move || {
                let connected = transport.connect(events);
                let failed = connected.is_err();
                let _ = ready_tx.send(connected);

                if failed {
                    return;
                }

                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        Cmd::Invoke {
                            method,
                            args,
                            timeout,
                            reply,
                        } => {
                            let result = transport
                                .invoke(&method, &args.to_string(), timeout)
                                .and_then(|json| {
                                    serde_json::from_str(&json).map_err(|e| {
                                        IpcError::Decode(format!(
                                            "result of {method} is not JSON: {e}"
                                        ))
                                    })
                                });
                            let _ = reply.send(result);
                        }
                        Cmd::Shutdown => break,
                    }
                }

                transport.disconnect();
            })
            .map_err(|e| {
                IpcError::Create(format!("failed to spawn the logos thread: {e}"))
            })?;

        match ready_rx.await {
            Ok(Ok(())) => Ok(Self {
                cmd_tx,
                join: Some(join),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(IpcError::ChannelClosed),
        }
    }

    /// Sends one blocking invoke to the logos thread and awaits the reply.
    /// `args` is the JSON array of arguments (anything else is a caller
    /// bug, debug-asserted).
    pub async fn invoke(
        &self,
        method: &str,
        args: Value,
        timeout: Duration,
    ) -> Result<Value, IpcError> {
        debug_assert!(
            args.is_array(),
            "lp method arguments must be a JSON array"
        );

        let (reply_tx, reply_rx) = oneshot::channel();

        self.cmd_tx
            .send(Cmd::Invoke {
                method: method.to_owned(),
                args,
                timeout,
                reply: reply_tx,
            })
            .map_err(|_| IpcError::ChannelClosed)?;

        match tokio::time::timeout(timeout + OUTER_TIMEOUT_GRACE, reply_rx)
            .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(IpcError::ChannelClosed),
            Err(_) => Err(IpcError::Timeout {
                method: method.to_owned(),
            }),
        }
    }

    /// Disconnects on the logos thread and joins it (bounded by
    /// [`JOIN_DEADLINE`], detaching past it). The join runs on a throwaway
    /// std thread rather than tokio's blocking pool so that a wedged logos
    /// thread is truly detached: nothing is left on the pool that runtime
    /// shutdown would wait for.
    pub async fn shutdown(mut self) {
        let _ = self.cmd_tx.send(Cmd::Shutdown);

        let Some(join) = self.join.take() else {
            return;
        };

        let (done_tx, done_rx) = oneshot::channel();
        std::thread::spawn(move || {
            let _ = join.join();
            let _ = done_tx.send(());
        });

        if tokio::time::timeout(JOIN_DEADLINE, done_rx).await.is_err() {
            log::warn!(
                "logos thread did not stop within {JOIN_DEADLINE:?}; detaching"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::fake::FakeTransport;

    #[tokio::test]
    async fn invoke_round_trips_and_serializes_args() {
        let (transport, controller) = FakeTransport::new();
        controller.respond("echo", "42");
        let (events_tx, _events_rx) = mpsc::unbounded_channel();
        let handle = LogosHandle::spawn(transport, events_tx).await.unwrap();

        let result = handle
            .invoke("echo", json!([1, "a"]), Duration::from_secs(1))
            .await
            .unwrap();

        assert_eq!(result, json!(42));
        assert_eq!(
            controller.calls(),
            vec![("echo".to_owned(), r#"[1,"a"]"#.to_owned())]
        );

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn concurrent_invokes_are_serialized_in_send_order() {
        let (transport, controller) = FakeTransport::new();
        let in_flight = Arc::new(AtomicBool::new(false));
        let overlaps = Arc::new(AtomicUsize::new(0));
        {
            let in_flight = in_flight.clone();
            let overlaps = overlaps.clone();
            controller.respond_with("work", move |_| {
                if in_flight.swap(true, Ordering::SeqCst) {
                    overlaps.fetch_add(1, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_millis(5));
                in_flight.store(false, Ordering::SeqCst);
                Ok("true".to_owned())
            });
        }
        let (events_tx, _events_rx) = mpsc::unbounded_channel();
        let handle =
            Arc::new(LogosHandle::spawn(transport, events_tx).await.unwrap());

        // On the current-thread test runtime, tasks are first polled in
        // spawn order, and `invoke` sends its command before first yielding,
        // so the send order below is deterministic.
        let tasks: Vec<_> = (0..4)
            .map(|i| {
                let handle = handle.clone();
                tokio::spawn(async move {
                    handle
                        .invoke("work", json!([i]), Duration::from_secs(5))
                        .await
                        .unwrap();
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(overlaps.load(Ordering::SeqCst), 0);
        assert_eq!(
            controller.calls(),
            (0..4)
                .map(|i| ("work".to_owned(), format!("[{i}]")))
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn outer_timeout_fires_when_the_thread_is_wedged() {
        let (transport, controller) = FakeTransport::new();
        controller.respond_with("stall", |_| {
            std::thread::sleep(Duration::from_secs(60));
            Ok("true".to_owned())
        });
        let (events_tx, _events_rx) = mpsc::unbounded_channel();
        let handle = LogosHandle::spawn(transport, events_tx).await.unwrap();

        let result = handle
            .invoke("stall", json!([]), Duration::from_millis(10))
            .await;

        assert!(
            matches!(result, Err(IpcError::Timeout { method }) if method == "stall")
        );
    }

    #[tokio::test]
    async fn events_flow_after_connect_and_before_any_invoke() {
        let (transport, controller) = FakeTransport::new();
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let handle = LogosHandle::spawn(transport, events_tx).await.unwrap();

        assert!(controller.connected());
        assert!(controller.calls().is_empty());

        controller
            .emit("module_event", r#"["chat_module","message_received"]"#);
        let event = events_rx.recv().await.unwrap();
        assert_eq!(event.name, "module_event");
        assert_eq!(event.payload_json, r#"["chat_module","message_received"]"#);

        handle.shutdown().await;
    }

    #[test]
    fn shutdown_with_a_wedged_thread_detaches_and_frees_the_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();

        let (transport, controller) = FakeTransport::new();
        controller.respond_with("stall", |_| {
            std::thread::sleep(Duration::from_secs(60));
            Ok("true".to_owned())
        });

        runtime.block_on(async {
            let (events_tx, _events_rx) = mpsc::unbounded_channel();
            let handle =
                LogosHandle::spawn(transport, events_tx).await.unwrap();

            let result = handle
                .invoke("stall", json!([]), Duration::from_millis(10))
                .await;
            assert!(matches!(result, Err(IpcError::Timeout { .. })));

            handle.shutdown().await;
        });

        // The detached join must leave nothing on the blocking pool:
        // dropping the runtime may not wait for the wedged logos thread.
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(runtime);
            let _ = dropped_tx.send(());
        });
        assert!(
            dropped_rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "runtime drop blocked on the detached logos thread"
        );
    }

    #[tokio::test]
    async fn shutdown_joins_and_disconnects_the_transport() {
        let (transport, controller) = FakeTransport::new();
        let (events_tx, _events_rx) = mpsc::unbounded_channel();
        let handle = LogosHandle::spawn(transport, events_tx).await.unwrap();
        assert!(controller.connected());

        handle.shutdown().await;

        assert!(!controller.connected());
    }
}
