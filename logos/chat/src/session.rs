//! The backend session state machine — the analogue of halloy's
//! `data::stream::run`. One session per app run; the returned stream's
//! FIRST item is always `Update::Controller` carrying the control sender.
//!
//! Startup ladder (the QML ChatBackend ordering, transposed):
//! daemon up → abi check → logos thread + `module_event` subscription →
//! `get_status` link check → `loadModule("chat_module")` →
//! `watchModuleEvents` per event (×7 explicit, or one wildcard behind
//! config) → `chat.init({delivery_preset})` → `list_conversations` snapshot →
//! `chat.status()` seeded through the SAME handler as the live event so an
//! already-online module triggers the resync path instead of being
//! swallowed.
//!
//! Main loop rules:
//! - Control channel (bounded, cap 32) is polled only when no action is in
//!   flight — natural serialization for the single-dispatch module. Events
//!   are always polled (they keep flowing during a slow 20s send).
//! - `conversation_updated` marks a resync and `members_changed` queues a
//!   member reload for its conversation — the events carry only the id, so
//!   the session refetches and the results arrive as
//!   `ConversationsSnapshot`/`MembersLoaded` (QML rehydrate parity).
//! - Mutating actions are gated on Online → `ActionFailed(NotOnline)`.
//! - A 5s health tick probes the daemon at the PROCESS level (never an lp
//!   call) so detection works while the logos thread is blocked.
//! - Fresh transition to Online after the initial snapshot → full resync
//!   (`Ready` + `ConversationsSnapshot` re-emitted).
//! - Dead link (`IpcError::DeadLink` or outer timeout) → check daemon →
//!   full restart ladder with a fresh daemon (1s/5s/15s backoff, then
//!   `Fatal(RestartExhausted)`).
//!
//! Shutdown (`Control::Quit`): `chat.shutdown()` (≤5s) →
//! `gateway.shutdown_daemon()` (≤3s, errors tolerated) →
//! `Supervisor::stop(grace)` → `LogosHandle::shutdown()` →
//! `Update::Stopped`, then pend forever (iced drops the stream).
//!
//! Pinned-down details the tests rely on:
//! - A live `delivery_state_changed` is forwarded as `Update::Event`
//!   BEFORE the delivery handler emits the phase; the seeded `status()`
//!   emits no `Update::Event` (it is not a wire event), only the phase
//!   (and, when already online, the resync).
//! - A delivery state of `initialising` (re-)emits
//!   `Phase(InitialisingChat)`; an unknown state keeps the previous phase
//!   and is only logged.
//! - Fatal errors emit `Phase(Failed)` then `Update::Fatal`, and the
//!   stream pends forever afterwards, exactly like after `Stopped`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;
use futures::stream::{self, Stream, StreamExt};
use logos_client::{
    FakeTransport, Gateway, GatewayError, IpcError, LogosHandle, RawEvent,
    Transport, decode_module_event,
};
use logos_daemon::Supervisor;
use tokio::sync::mpsc as tokio_mpsc;

use crate::client::{ChatClient, MODULE};
use crate::config::BackendConfig;
use crate::error::{ActionError, BackendError, ChatError, LoadKind};
use crate::event::ChatEvent;
use crate::types::{
    Conversation, ConvoId, DeliveryState, GroupMember, Message,
};

/// Capacity of the control channel; senders see backpressure past this.
pub const CONTROL_CAP: usize = 32;

/// Process-level daemon liveness probe period (Live driver only).
const HEALTH_TICK: Duration = Duration::from_secs(5);
/// Outer cap on `chat_module.shutdown()` during a clean quit.
const CHAT_SHUTDOWN_CAP: Duration = Duration::from_secs(5);
/// Outer cap on `core_service.shutdown` during a clean quit.
const DAEMON_SHUTDOWN_CAP: Duration = Duration::from_secs(3);
/// Grace given to the daemon child before SIGTERM/SIGKILL escalation.
const SUPERVISOR_STOP_GRACE: Duration = Duration::from_secs(3);

/// Backoff before restart attempts 1, 2, and 3+ respectively.
#[cfg(not(test))]
const RESTART_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
];
/// Test builds shorten the backoff: the logos thread is a real OS thread,
/// so tests run on real (tiny) durations rather than paused time.
#[cfg(test)]
const RESTART_BACKOFF: [Duration; 3] = [
    Duration::from_millis(5),
    Duration::from_millis(10),
    Duration::from_millis(15),
];

/// What the session runs against.
pub enum Driver {
    /// Full daemon supervision + lp_* FFI transport. Requires the `ffi`
    /// feature; without it the session ends immediately with
    /// `Fatal(FfiUnavailable)`.
    Live,
    /// Scripted transports — unit tests and the UI's mock mode. Daemon
    /// supervision is skipped entirely; phases are still emitted. The
    /// factory is called once per (re)start attempt, so restart behavior is
    /// testable.
    Fake(Box<dyn FnMut() -> FakeTransport + Send>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    StartingDaemon,
    Connecting,
    LoadingModule,
    InitialisingChat,
    Online,
    DeliveryError { detail: String },
    DeliveryStopped,
    Restarting { attempt: u32 },
    ShuttingDown,
    Failed,
}

#[derive(Debug)]
pub enum Update {
    /// Always the first item.
    Controller(mpsc::Sender<Control>),
    Phase(Phase),
    /// First successful `get_address` (and again after each resync).
    /// `installation_name` is `None` while unset (or unavailable).
    Ready {
        my_address: String,
        installation_name: Option<String>,
    },
    /// Initial snapshot AND every resync: replaces all conversation state.
    ConversationsSnapshot(Vec<Conversation>),
    MessagesLoaded {
        convo_id: ConvoId,
        messages: Vec<Message>,
    },
    MembersLoaded {
        convo_id: ConvoId,
        members: Vec<GroupMember>,
    },
    /// Live push event, already decoded.
    Event(ChatEvent),
    ActionFailed(ActionError),
    /// Terminal for this run.
    Fatal(BackendError),
    /// Final item after a clean shutdown.
    Stopped,
}

#[derive(Debug, Clone)]
pub enum Control {
    SendMessage {
        convo_id: ConvoId,
        content: String,
    },
    CreateConversation {
        peer_address: String,
    },
    CreateGroup {
        name: String,
        description: String,
    },
    AddGroupMember {
        convo_id: ConvoId,
        peer_address: String,
    },
    LoadMessages {
        convo_id: ConvoId,
    },
    LoadMembers {
        convo_id: ConvoId,
    },
    SetNickname {
        convo_id: ConvoId,
        nickname: String,
    },
    DeleteConversation {
        convo_id: ConvoId,
    },
    /// Manual full refetch; coalesced if one is already pending.
    Resync,
    /// Clean shutdown; the stream ends with `Update::Stopped`.
    Quit,
}

/// Runs the session. Mirrors halloy's `data::stream::run` shape: a spawned
/// tokio task feeding an unbounded channel, so backend progress is never
/// subject to iced's subscription backpressure.
pub fn run(
    config: BackendConfig,
    driver: Driver,
) -> impl Stream<Item = Update> {
    let (tx, rx) = mpsc::unbounded();
    let runner = stream::once(async move {
        session(config, driver, tx).await;
    })
    .filter_map(|()| async { None::<Update> });
    stream::select(rx, runner)
}

async fn session(
    config: BackendConfig,
    mut driver: Driver,
    updates: mpsc::UnboundedSender<Update>,
) {
    let (control_tx, mut control_rx) = mpsc::channel(CONTROL_CAP);
    let mut session = Session {
        config,
        updates,
        delivery: DeliveryState::Initialising,
        initial_snapshot_done: false,
        resync_dirty: false,
        pending_member_loads: Vec::new(),
    };
    session.emit(Update::Controller(control_tx));

    let mut attempt: u32 = 0;
    let mut last_error: Option<String>;

    loop {
        match start_stack(&mut session, &mut driver).await {
            Ok(mut stack) => {
                attempt = 0;
                match main_loop(&mut session, &mut stack, &mut control_rx).await
                {
                    LoopExit::Quit => return quit(&session, stack).await,
                    LoopExit::Dead(reason) => {
                        log::warn!("backend link died: {reason}");
                        last_error = Some(reason);
                        teardown(stack).await;
                    }
                }
            }
            Err(error) => {
                if attempt == 0 {
                    return fatal(&session, error).await;
                }
                log::warn!("restart attempt {attempt} failed: {error}");
                last_error = Some(error.to_string());
            }
        }

        attempt += 1;
        if attempt > session.config.restart_max_attempts {
            let error = BackendError::RestartExhausted {
                attempts: attempt - 1,
                last: last_error.take().unwrap_or_default(),
            };
            return fatal(&session, error).await;
        }
        session.phase(Phase::Restarting { attempt });
        tokio::time::sleep(restart_backoff(attempt)).await;
    }
}

/// A boxed in-flight action. It emits its own updates; `Err` carries a
/// dead-link reason that aborts the run into the restart ladder.
type ActionFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

enum LoopExit {
    Quit,
    Dead(String),
}

enum Applied {
    Handled,
    Quit,
}

enum Arm {
    Raw(Option<RawEvent>),
    Action(Result<(), String>),
    Control(Option<Control>),
    Health,
}

/// Session-wide mutable state plus the update sender.
struct Session {
    config: BackendConfig,
    updates: mpsc::UnboundedSender<Update>,
    delivery: DeliveryState,
    initial_snapshot_done: bool,
    resync_dirty: bool,
    pending_member_loads: Vec<ConvoId>,
}

impl Session {
    fn emit(&self, update: Update) {
        let _ = self.updates.unbounded_send(update);
    }

    fn phase(&self, phase: Phase) {
        self.emit(Update::Phase(phase));
    }

    /// THE delivery-state handler: both the live `delivery_state_changed`
    /// event and the seeded `status()` route through here, so an
    /// already-online module still triggers the fresh-online resync. The
    /// resync itself is deferred through the dirty flag and runs as the
    /// next in-flight action.
    fn handle_delivery(&mut self, state: DeliveryState, detail: &str) {
        match state {
            DeliveryState::Online => {
                let fresh = self.delivery != DeliveryState::Online;
                self.delivery = DeliveryState::Online;
                self.phase(Phase::Online);
                if fresh && self.initial_snapshot_done {
                    self.resync_dirty = true;
                }
            }
            DeliveryState::Error => {
                self.delivery = DeliveryState::Error;
                self.phase(Phase::DeliveryError {
                    detail: detail.to_owned(),
                });
            }
            DeliveryState::Stopped => {
                self.delivery = DeliveryState::Stopped;
                self.phase(Phase::DeliveryStopped);
            }
            DeliveryState::Initialising => {
                self.delivery = DeliveryState::Initialising;
                self.phase(Phase::InitialisingChat);
            }
            DeliveryState::Unknown => {
                log::warn!(
                    "unknown delivery state (detail {detail:?}); keeping \
                     the previous state"
                );
            }
        }
    }
}

/// One connected backend stack, torn down and rebuilt on every restart.
struct Stack {
    chat: ChatClient,
    gateway: Gateway,
    handle: Arc<LogosHandle>,
    events: tokio_mpsc::UnboundedReceiver<RawEvent>,
    supervisor: Option<Supervisor>,
}

/// Runs the startup ladder for the configured driver. Resets the per-run
/// delivery state first: a restart means a fresh daemon and module.
async fn start_stack(
    session: &mut Session,
    driver: &mut Driver,
) -> Result<Stack, BackendError> {
    session.delivery = DeliveryState::Initialising;
    session.initial_snapshot_done = false;
    session.resync_dirty = false;
    session.pending_member_loads.clear();

    match driver {
        Driver::Live => start_live(session).await,
        Driver::Fake(factory) => {
            session.phase(Phase::StartingDaemon);
            let transport = factory();
            connect_stack(session, transport, None).await
        }
    }
}

#[cfg(feature = "ffi")]
async fn start_live(session: &mut Session) -> Result<Stack, BackendError> {
    use logos_client::ffi_transport::{FfiConfig, FfiTransport};

    session.phase(Phase::StartingDaemon);

    let artifacts = logos_daemon::locate(&session.config.artifacts)
        .map_err(locate_error)?;
    let config_dir = session.config.daemon_config_dir();

    if session.config.reap_stale_daemon {
        logos_daemon::reap_stale(&artifacts, &config_dir)
            .await
            .map_err(daemon_error)?;
    } else {
        log::warn!(
            "starting without the single-instance guard: leaving any daemon \
             holding {} alone, so a sibling instance keeps its backend",
            config_dir.display()
        );
    }

    let supervisor = Supervisor::start(&artifacts, &config_dir)
        .await
        .map_err(daemon_error)?;

    let ports = supervisor.ports();
    let transport = FfiTransport::new(FfiConfig {
        token: supervisor.token().to_owned(),
        origin: session.config.origin.clone(),
        core_service_port: ports.core_service,
        capability_port: ports.capability_module,
    });

    connect_stack(session, transport, Some(supervisor)).await
}

#[cfg(not(feature = "ffi"))]
async fn start_live(_session: &mut Session) -> Result<Stack, BackendError> {
    Err(BackendError::FfiUnavailable)
}

/// The transport-independent tail of the ladder: connect (which arms the
/// `module_event` subscription), link check, module load, watches, chat
/// init, snapshot, and the status seed.
async fn connect_stack(
    session: &mut Session,
    transport: impl Transport,
    supervisor: Option<Supervisor>,
) -> Result<Stack, BackendError> {
    session.phase(Phase::Connecting);

    let (events_tx, events) = tokio_mpsc::unbounded_channel();
    let handle = Arc::new(
        LogosHandle::spawn(transport, events_tx)
            .await
            .map_err(|e| BackendError::ClientCreateFailed(e.to_string()))?,
    );
    let gateway = Gateway::new(Arc::clone(&handle));
    let chat = ChatClient::new(gateway.clone());

    gateway.get_status().await.map_err(|e| {
        BackendError::DaemonUnhealthy(format!("getStatus failed: {e}"))
    })?;

    session.phase(Phase::LoadingModule);
    gateway
        .load_module(MODULE)
        .await
        .map_err(|e| BackendError::ModuleLoadFailed(e.to_string()))?;

    watch_events(session, &gateway).await?;

    session.phase(Phase::InitialisingChat);
    chat.init(&session.config.delivery_preset)
        .await
        .map_err(|e| BackendError::ChatInitFailed(e.to_string()))?;

    // Best-effort cosmetics: a failure never blocks the ladder (a dead
    // link would surface on the snapshot right after anyway).
    if let Some(name) = session
        .config
        .installation_name
        .as_deref()
        .filter(|name| !name.is_empty())
        && let Err(error) = chat.set_installation_name(name).await
    {
        log::warn!("set_installation_name failed: {error}");
    }

    let conversations = chat.list_conversations().await.map_err(|e| {
        BackendError::ChatInitFailed(format!("initial snapshot failed: {e}"))
    })?;
    session.emit(Update::ConversationsSnapshot(conversations));
    session.initial_snapshot_done = true;

    let status = chat.status().await.map_err(|e| {
        BackendError::ChatInitFailed(format!("status seeding failed: {e}"))
    })?;
    session.handle_delivery(status.delivery_state, &status.detail);

    Ok(Stack {
        chat,
        gateway,
        handle,
        events,
        supervisor,
    })
}

/// Registers the event watches — exactly once per daemon run (there is no
/// unwatch, and re-watching duplicates forwarders).
async fn watch_events(
    session: &Session,
    gateway: &Gateway,
) -> Result<(), BackendError> {
    let names: &[&str] = if session.config.use_wildcard_watch {
        &[""]
    } else {
        &ChatEvent::NAMES
    };
    for name in names.iter().copied() {
        let watched =
            gateway
                .watch_module_events(MODULE, name)
                .await
                .map_err(|e| BackendError::WatchFailed {
                    event: name.to_owned(),
                    detail: e.to_string(),
                })?;
        if !watched {
            return Err(BackendError::WatchFailed {
                event: name.to_owned(),
                detail: "daemon did not register the watch".to_owned(),
            });
        }
    }
    Ok(())
}

async fn main_loop(
    session: &mut Session,
    stack: &mut Stack,
    control_rx: &mut mpsc::Receiver<Control>,
) -> LoopExit {
    let mut in_flight: Option<ActionFuture> = None;
    let mut health = tokio::time::interval(HEALTH_TICK);

    loop {
        if in_flight.is_none()
            && (session.resync_dirty
                || !session.pending_member_loads.is_empty())
        {
            match control_rx.try_recv() {
                Ok(control) => {
                    match apply_control(session, stack, &mut in_flight, control)
                    {
                        Applied::Handled => {}
                        Applied::Quit => return LoopExit::Quit,
                    }
                    continue;
                }
                Err(mpsc::TryRecvError::Closed) => return LoopExit::Quit,
                Err(mpsc::TryRecvError::Empty) => {
                    if session.resync_dirty {
                        session.resync_dirty = false;
                        in_flight =
                            Some(resync_action(&stack.chat, &session.updates));
                    } else {
                        let convo_id = session.pending_member_loads.remove(0);
                        in_flight = Some(action(
                            &stack.chat,
                            &session.updates,
                            Control::LoadMembers { convo_id },
                        ));
                    }
                }
            }
        }

        let arm = tokio::select! {
            raw = stack.events.recv() => Arm::Raw(raw),
            outcome = async {
                in_flight
                    .as_mut()
                    .expect("in-flight action gated by the branch condition")
                    .await
            }, if in_flight.is_some() => Arm::Action(outcome),
            control = control_rx.next(), if in_flight.is_none() => {
                Arm::Control(control)
            }
            _ = health.tick(), if stack.supervisor.is_some() => Arm::Health,
        };

        match arm {
            Arm::Raw(Some(raw)) => handle_raw_event(session, &raw),
            Arm::Raw(None) => {
                return LoopExit::Dead("event channel closed".to_owned());
            }
            Arm::Action(outcome) => {
                in_flight = None;
                if let Err(reason) = outcome {
                    return LoopExit::Dead(reason);
                }
            }
            Arm::Control(Some(control)) => {
                match apply_control(session, stack, &mut in_flight, control) {
                    Applied::Handled => {}
                    Applied::Quit => return LoopExit::Quit,
                }
            }
            Arm::Control(None) => return LoopExit::Quit,
            Arm::Health => {
                if let Some(supervisor) = stack.supervisor.as_mut()
                    && !supervisor.is_alive().await
                {
                    return LoopExit::Dead(
                        "logoscore process exited".to_owned(),
                    );
                }
            }
        }
    }
}

/// Decodes one wire event: filter to chat_module, forward the raw
/// `Update::Event`, then feed `delivery_state_changed` into the delivery
/// handler. Undecodable payloads are logged, never fatal.
fn handle_raw_event(session: &mut Session, raw: &RawEvent) {
    if raw.name != "module_event" {
        log::debug!("ignoring unexpected event {}", raw.name);
        return;
    }
    let module_event = match decode_module_event(raw) {
        Ok(module_event) => module_event,
        Err(error) => {
            log::warn!("undecodable module event: {error}");
            return;
        }
    };
    if module_event.module != MODULE {
        return;
    }
    match ChatEvent::decode(&module_event.event, &module_event.args) {
        Ok(event) => {
            let delivery = match &event {
                ChatEvent::DeliveryStateChanged { state, detail } => {
                    Some((*state, detail.clone()))
                }
                _ => None,
            };
            match &event {
                // These carry only the id: refetch, so the real state
                // arrives as `ConversationsSnapshot`/`MembersLoaded`.
                ChatEvent::ConversationUpdated { .. } => {
                    session.resync_dirty = true;
                }
                ChatEvent::MembersChanged { convo_id } => {
                    if !session.pending_member_loads.contains(convo_id) {
                        session.pending_member_loads.push(convo_id.clone());
                    }
                }
                _ => {}
            }
            session.emit(Update::Event(event));
            if let Some((state, detail)) = delivery {
                session.handle_delivery(state, &detail);
            }
        }
        Err(error) => log::warn!("undecodable chat event: {error}"),
    }
}

fn apply_control(
    session: &mut Session,
    stack: &Stack,
    in_flight: &mut Option<ActionFuture>,
    control: Control,
) -> Applied {
    match control {
        Control::Quit => return Applied::Quit,
        Control::Resync => {
            session.resync_dirty = true;
            return Applied::Handled;
        }
        _ => {}
    }

    if let Some(attempted) = gated_method(&control)
        && !session.delivery.is_online()
    {
        session
            .emit(Update::ActionFailed(ActionError::NotOnline { attempted }));
        return Applied::Handled;
    }

    *in_flight = Some(action(&stack.chat, &session.updates, control));
    Applied::Handled
}

/// The mutating controls, gated on delivery being online; read-only loads
/// and resyncs run regardless (the module's local store answers offline).
fn gated_method(control: &Control) -> Option<&'static str> {
    match control {
        Control::SendMessage { .. } => Some("send_message"),
        Control::CreateConversation { .. } => Some("create_conversation"),
        Control::CreateGroup { .. } => Some("create_group_conversation"),
        Control::AddGroupMember { .. } => Some("add_group_member"),
        Control::SetNickname { .. } => Some("set_conversation_nickname"),
        Control::DeleteConversation { .. } => Some("delete_conversation"),
        Control::LoadMessages { .. }
        | Control::LoadMembers { .. }
        | Control::Resync
        | Control::Quit => None,
    }
}

/// Builds the in-flight future for one action. Successful creates emit
/// nothing extra — the `conversation_created` push event drives the UI.
fn action(
    chat: &ChatClient,
    updates: &mpsc::UnboundedSender<Update>,
    control: Control,
) -> ActionFuture {
    let chat = chat.clone();
    let updates = updates.clone();
    Box::pin(async move {
        let failure = match control {
            Control::SendMessage { convo_id, content } => {
                match chat.send_message(&convo_id, &content).await {
                    Ok(()) => None,
                    Err(error) => Some((
                        ActionError::SendFailed {
                            convo_id,
                            content,
                            reason: error.to_string(),
                        },
                        error,
                    )),
                }
            }
            Control::CreateConversation { peer_address } => {
                match chat.create_conversation(&peer_address).await {
                    Ok(_) => None,
                    Err(error) => Some((
                        module_error("create_conversation", &error),
                        error,
                    )),
                }
            }
            Control::CreateGroup { name, description } => {
                match chat.create_group_conversation(&name, &description).await
                {
                    Ok(_) => None,
                    Err(error) => Some((
                        module_error("create_group_conversation", &error),
                        error,
                    )),
                }
            }
            Control::AddGroupMember {
                convo_id,
                peer_address,
            } => match chat.add_group_member(&convo_id, &peer_address).await {
                Ok(()) => None,
                Err(error) => {
                    Some((module_error("add_group_member", &error), error))
                }
            },
            Control::LoadMessages { convo_id } => {
                match chat.get_messages(&convo_id).await {
                    Ok(messages) => {
                        let _ =
                            updates.unbounded_send(Update::MessagesLoaded {
                                convo_id,
                                messages,
                            });
                        None
                    }
                    Err(error) => Some((
                        ActionError::LoadFailed {
                            convo_id,
                            what: LoadKind::Messages,
                            reason: error.to_string(),
                        },
                        error,
                    )),
                }
            }
            Control::LoadMembers { convo_id } => {
                match chat.list_group_members(&convo_id).await {
                    Ok(members) => {
                        let _ = updates.unbounded_send(Update::MembersLoaded {
                            convo_id,
                            members,
                        });
                        None
                    }
                    Err(error) => Some((
                        ActionError::LoadFailed {
                            convo_id,
                            what: LoadKind::Members,
                            reason: error.to_string(),
                        },
                        error,
                    )),
                }
            }
            Control::SetNickname { convo_id, nickname } => {
                match chat.set_conversation_nickname(&convo_id, &nickname).await
                {
                    Ok(()) => None,
                    Err(error) => Some((
                        module_error("set_conversation_nickname", &error),
                        error,
                    )),
                }
            }
            Control::DeleteConversation { convo_id } => {
                match chat.delete_conversation(&convo_id).await {
                    Ok(()) => None,
                    Err(error) => Some((
                        module_error("delete_conversation", &error),
                        error,
                    )),
                }
            }
            // Handled in `apply_control`, never dispatched here.
            Control::Resync | Control::Quit => None,
        };

        match failure {
            None => Ok(()),
            Some((action_error, error)) => {
                let dead = dead_link(&error);
                let _ =
                    updates.unbounded_send(Update::ActionFailed(action_error));
                if dead { Err(error.to_string()) } else { Ok(()) }
            }
        }
    })
}

/// The full resync: address + installation name (→ `Ready`) then snapshot
/// (→ `ConversationsSnapshot`). Serves both the fresh-online recovery
/// refetch and manual `Control::Resync`s, coalesced by the dirty flag.
fn resync_action(
    chat: &ChatClient,
    updates: &mpsc::UnboundedSender<Update>,
) -> ActionFuture {
    let chat = chat.clone();
    let updates = updates.clone();
    Box::pin(async move {
        match chat.get_address().await {
            Ok(my_address) => {
                let installation_name = match chat.get_installation_name().await
                {
                    Ok(name) if !name.is_empty() => Some(name),
                    Ok(_) => None,
                    Err(error) if dead_link(&error) => {
                        return Err(error.to_string());
                    }
                    Err(error) => {
                        log::warn!(
                            "get_installation_name failed during resync: \
                             {error}"
                        );
                        None
                    }
                };
                let _ = updates.unbounded_send(Update::Ready {
                    my_address,
                    installation_name,
                });
            }
            Err(error) if dead_link(&error) => {
                return Err(error.to_string());
            }
            Err(error) => {
                log::warn!("get_address failed during resync: {error}");
            }
        }
        match chat.list_conversations().await {
            Ok(conversations) => {
                let _ = updates.unbounded_send(Update::ConversationsSnapshot(
                    conversations,
                ));
            }
            Err(error) if dead_link(&error) => {
                return Err(error.to_string());
            }
            Err(error) => {
                log::warn!("list_conversations failed during resync: {error}");
            }
        }
        Ok(())
    })
}

fn module_error(method: &'static str, error: &ChatError) -> ActionError {
    ActionError::Module {
        method,
        reason: error.to_string(),
    }
}

/// Dead-link suspicion: the daemon-death `null` signature or the outer
/// timeout — either way this stack is done and must be rebuilt.
fn dead_link(error: &ChatError) -> bool {
    matches!(
        error,
        ChatError::Gateway(GatewayError::Ipc(
            IpcError::DeadLink | IpcError::Timeout { .. }
        ))
    )
}

fn restart_backoff(attempt: u32) -> Duration {
    let index = attempt.saturating_sub(1) as usize;
    RESTART_BACKOFF[index.min(RESTART_BACKOFF.len() - 1)]
}

/// Clean shutdown in contract order: chat shutdown → daemon shutdown →
/// supervisor stop → logos thread join → `Stopped`, then pend forever.
async fn quit(session: &Session, stack: Stack) {
    session.phase(Phase::ShuttingDown);

    let Stack {
        chat,
        gateway,
        handle,
        events,
        supervisor,
    } = stack;

    match tokio::time::timeout(CHAT_SHUTDOWN_CAP, chat.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            log::warn!("chat_module.shutdown failed: {error}");
        }
        Err(_) => log::warn!("chat_module.shutdown timed out"),
    }

    if tokio::time::timeout(DAEMON_SHUTDOWN_CAP, gateway.shutdown_daemon())
        .await
        .is_err()
    {
        log::warn!("core_service.shutdown timed out");
    }

    drop(chat);
    drop(gateway);
    drop(events);

    if let Some(supervisor) = supervisor {
        supervisor.stop(SUPERVISOR_STOP_GRACE).await;
    }

    shutdown_handle(handle).await;

    session.emit(Update::Stopped);
    std::future::pending::<()>().await;
}

/// Tears one stack down before a restart: drop every gateway clone so the
/// logos thread can be joined, then stop the daemon (fresh daemon per
/// run — re-watching on a surviving daemon would duplicate forwarders).
async fn teardown(stack: Stack) {
    let Stack {
        chat,
        gateway,
        handle,
        events,
        supervisor,
    } = stack;
    drop(chat);
    drop(gateway);
    drop(events);
    shutdown_handle(handle).await;
    if let Some(supervisor) = supervisor {
        supervisor.stop(Duration::ZERO).await;
    }
}

/// Joins the logos thread when this is the last reference; otherwise the
/// dropped command channel makes the thread disconnect and exit on its own.
async fn shutdown_handle(handle: Arc<LogosHandle>) {
    match Arc::try_unwrap(handle) {
        Ok(handle) => handle.shutdown().await,
        Err(_) => {
            log::warn!(
                "logos handle still shared at teardown; leaving the \
                 thread to exit on channel close"
            );
        }
    }
}

/// Emits the terminal `Phase(Failed)` + `Fatal` pair, then pends forever
/// (matching `Stopped`: the consumer drops the stream when it is done).
async fn fatal(session: &Session, error: BackendError) {
    session.phase(Phase::Failed);
    session.emit(Update::Fatal(error));
    std::future::pending::<()>().await;
}

#[cfg(feature = "ffi")]
fn locate_error(error: logos_daemon::LocateError) -> BackendError {
    let logos_daemon::LocateError::NotFound { artifact, searched } = error;
    BackendError::ArtifactsMissing {
        what: match artifact {
            logos_daemon::Artifact::LogoscoreBinary => {
                "logoscore binary".to_owned()
            }
            logos_daemon::Artifact::ModulesDir => {
                "modules directory".to_owned()
            }
        },
        searched,
    }
}

#[cfg(feature = "ffi")]
fn daemon_error(error: logos_daemon::DaemonError) -> BackendError {
    match error {
        logos_daemon::DaemonError::Locate(error) => locate_error(error),
        logos_daemon::DaemonError::Spawn(detail) => {
            BackendError::DaemonStartFailed(detail)
        }
        logos_daemon::DaemonError::Unhealthy(window) => {
            BackendError::DaemonUnhealthy(format!(
                "not healthy within {window:?}"
            ))
        }
        logos_daemon::DaemonError::Token(detail) => {
            BackendError::TokenFailed(detail)
        }
        logos_daemon::DaemonError::State(detail) => {
            BackendError::DaemonStartFailed(detail)
        }
        logos_daemon::DaemonError::Io(error) => {
            BackendError::DaemonStartFailed(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use logos_client::fake::FakeController;
    use serde_json::{Value, json};

    use super::*;

    const NEXT_TIMEOUT: Duration = Duration::from_secs(10);

    fn config() -> BackendConfig {
        BackendConfig::new(std::env::temp_dir())
    }

    fn single(transport: FakeTransport) -> Driver {
        let mut slot = Some(transport);
        Driver::Fake(Box::new(move || {
            slot.take().expect("factory called more than once")
        }))
    }

    /// Scripts a full happy-path ladder. `delivery` is what `status()`
    /// reports at seed time; `overrides` replace the default result of a
    /// chat-level method.
    fn script_ladder(
        controller: &FakeController,
        delivery: &'static str,
        overrides: &[(&'static str, Value)],
    ) {
        controller.respond("getStatus", r#"{"modules":["chat_module"]}"#);
        controller
            .respond("loadModule", r#"{"status":"ok","module":"chat_module"}"#);
        controller.respond("watchModuleEvents", "true");

        let overrides = overrides.to_vec();
        controller.respond_with("callModuleMethod", move |args| {
            let args: Value =
                serde_json::from_str(args).expect("call args are JSON");
            let method = args[1].as_str().expect("method name").to_owned();
            let result = overrides
                .iter()
                .find(|(name, _)| *name == method)
                .map_or_else(
                    || default_result(&method, delivery),
                    |(_, value)| value.clone(),
                );
            Ok(json!({
                "status": "ok",
                "module": "chat_module",
                "method": method,
                "result": result,
            })
            .to_string())
        });
    }

    fn default_result(method: &str, delivery: &str) -> Value {
        match method {
            "init"
            | "shutdown"
            | "send_message"
            | "add_group_member"
            | "set_conversation_nickname"
            | "set_installation_name"
            | "delete_conversation" => {
                json!({"success": true, "value": null, "error": null})
            }
            "create_conversation" | "create_group_conversation" => {
                json!({"success": true, "value": "convo-new", "error": null})
            }
            "list_conversations" | "get_messages" | "list_group_members" => {
                json!([])
            }
            "get_address" => json!("my-address"),
            "get_installation_name" => json!(""),
            "status" => json!({
                "convo_count": 0,
                "delivery_state": delivery,
                "detail": "",
            }),
            other => panic!("unscripted chat method {other}"),
        }
    }

    async fn next(stream: &mut (impl Stream<Item = Update> + Unpin)) -> Update {
        tokio::time::timeout(NEXT_TIMEOUT, stream.next())
            .await
            .expect("timed out waiting for an update")
            .expect("update stream ended")
    }

    async fn controller_of(
        stream: &mut (impl Stream<Item = Update> + Unpin),
    ) -> mpsc::Sender<Control> {
        match next(stream).await {
            Update::Controller(sender) => sender,
            other => panic!("first update was {other:?}, not Controller"),
        }
    }

    /// Collects updates until `pred` matches, returning everything
    /// received including the match.
    async fn collect_until(
        stream: &mut (impl Stream<Item = Update> + Unpin),
        pred: impl Fn(&Update) -> bool,
    ) -> Vec<Update> {
        let mut updates = Vec::new();
        loop {
            let update = next(stream).await;
            let done = pred(&update);
            updates.push(update);
            if done {
                return updates;
            }
        }
    }

    fn phases(updates: &[Update]) -> Vec<&Phase> {
        updates
            .iter()
            .filter_map(|update| match update {
                Update::Phase(phase) => Some(phase),
                _ => None,
            })
            .collect()
    }

    /// Drives the ladder with a seeded `initialising` state and returns
    /// the control sender once the seed landed (main loop running).
    async fn start_initialising(
        stream: &mut (impl Stream<Item = Update> + Unpin),
    ) -> mpsc::Sender<Control> {
        let control = controller_of(stream).await;
        collect_until(stream, |update| {
            matches!(update, Update::ConversationsSnapshot(_))
        })
        .await;
        match next(stream).await {
            Update::Phase(Phase::InitialisingChat) => {}
            other => panic!("expected the seeded phase, got {other:?}"),
        }
        control
    }

    /// Drives the ladder with a seeded `online` state through the
    /// fresh-online resync and returns the control sender.
    async fn start_online(
        stream: &mut (impl Stream<Item = Update> + Unpin),
    ) -> mpsc::Sender<Control> {
        let control = controller_of(stream).await;
        collect_until(stream, |update| {
            matches!(update, Update::ConversationsSnapshot(_))
        })
        .await;
        match next(stream).await {
            Update::Phase(Phase::Online) => {}
            other => panic!("expected Phase(Online), got {other:?}"),
        }
        match next(stream).await {
            Update::Ready { .. } => {}
            other => panic!("expected Ready, got {other:?}"),
        }
        match next(stream).await {
            Update::ConversationsSnapshot(_) => {}
            other => panic!("expected the resync snapshot, got {other:?}"),
        }
        control
    }

    #[tokio::test]
    async fn ladder_emits_controller_first_and_calls_in_contract_order() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "initialising", &[]);
        let connected_at_first_call = Arc::new(AtomicBool::new(false));
        {
            let flag = connected_at_first_call.clone();
            let probe = controller.clone();
            controller.respond_with("getStatus", move |_| {
                flag.store(probe.connected(), Ordering::SeqCst);
                Ok(r#"{"modules":["chat_module"]}"#.to_owned())
            });
        }

        let mut stream = Box::pin(run(config(), single(transport)));
        let _control = controller_of(&mut stream).await;

        let updates = collect_until(&mut stream, |update| {
            matches!(update, Update::ConversationsSnapshot(_))
        })
        .await;
        assert_eq!(
            phases(&updates),
            vec![
                &Phase::StartingDaemon,
                &Phase::Connecting,
                &Phase::LoadingModule,
                &Phase::InitialisingChat,
            ]
        );
        match next(&mut stream).await {
            Update::Phase(Phase::InitialisingChat) => {}
            other => panic!("expected the seeded phase, got {other:?}"),
        }

        assert!(
            connected_at_first_call.load(Ordering::SeqCst),
            "the subscription must be armed before the first call"
        );

        let mut expected = vec![
            ("getStatus".to_owned(), "[]".to_owned()),
            ("loadModule".to_owned(), r#"["chat_module"]"#.to_owned()),
        ];
        expected.extend(ChatEvent::NAMES.iter().map(|name| {
            (
                "watchModuleEvents".to_owned(),
                format!(r#"["chat_module","{name}"]"#),
            )
        }));
        expected.extend([
            (
                "callModuleMethod".to_owned(),
                format!(
                    r#"["chat_module","init",[{{"delivery_preset":"{}"}}]]"#,
                    config().delivery_preset
                ),
            ),
            (
                "callModuleMethod".to_owned(),
                r#"["chat_module","list_conversations",[]]"#.to_owned(),
            ),
            (
                "callModuleMethod".to_owned(),
                r#"["chat_module","status",[]]"#.to_owned(),
            ),
        ]);
        assert_eq!(controller.calls(), expected);
    }

    #[tokio::test]
    async fn wildcard_watch_registers_a_single_empty_watch() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "initialising", &[]);
        let mut config = config();
        config.use_wildcard_watch = true;

        let mut stream = Box::pin(run(config, single(transport)));
        start_initialising(&mut stream).await;

        let watches: Vec<String> = controller
            .calls()
            .into_iter()
            .filter(|(method, _)| method == "watchModuleEvents")
            .map(|(_, args)| args)
            .collect();
        assert_eq!(watches, vec![r#"["chat_module",""]"#.to_owned()]);
    }

    #[tokio::test]
    async fn online_status_seed_fires_the_resync_exactly_once() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "online", &[]);

        let mut stream = Box::pin(run(config(), single(transport)));
        let control = controller_of(&mut stream).await;

        collect_until(&mut stream, |update| {
            matches!(update, Update::ConversationsSnapshot(_))
        })
        .await;
        match next(&mut stream).await {
            Update::Phase(Phase::Online) => {}
            other => panic!("expected Phase(Online), got {other:?}"),
        }
        match next(&mut stream).await {
            Update::Ready { my_address, .. } => {
                assert_eq!(my_address, "my-address");
            }
            other => panic!("expected Ready, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::ConversationsSnapshot(_) => {}
            other => panic!("expected the resync snapshot, got {other:?}"),
        }

        // A follow-up roundtrip proves no further Ready is pending, then
        // the call log confirms a single resync ran.
        let mut control = control;
        control
            .try_send(Control::LoadMessages {
                convo_id: ConvoId("c1".to_owned()),
            })
            .unwrap();
        match next(&mut stream).await {
            Update::MessagesLoaded { .. } => {}
            other => panic!("expected MessagesLoaded, got {other:?}"),
        }
        let address_calls = controller
            .calls()
            .iter()
            .filter(|(method, args)| {
                method == "callModuleMethod"
                    && args.contains(r#""get_address""#)
            })
            .count();
        assert_eq!(address_calls, 1);
    }

    #[tokio::test]
    async fn mutating_controls_are_gated_until_online() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "initialising", &[]);

        let mut stream = Box::pin(run(config(), single(transport)));
        let mut control = start_initialising(&mut stream).await;

        control
            .try_send(Control::SendMessage {
                convo_id: ConvoId("c1".to_owned()),
                content: "hello".to_owned(),
            })
            .unwrap();

        match next(&mut stream).await {
            Update::ActionFailed(ActionError::NotOnline { attempted }) => {
                assert_eq!(attempted, "send_message");
            }
            other => panic!("expected NotOnline, got {other:?}"),
        }
        assert!(
            controller
                .calls()
                .iter()
                .all(|(_, args)| !args.contains("send_message")),
            "a gated send must never reach the module"
        );
    }

    #[tokio::test]
    async fn send_failure_carries_the_composed_content() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(
            &controller,
            "online",
            &[(
                "send_message",
                json!({
                    "success": false,
                    "value": null,
                    "error": "mls exploded",
                }),
            )],
        );

        let mut stream = Box::pin(run(config(), single(transport)));
        let mut control = start_online(&mut stream).await;

        control
            .try_send(Control::SendMessage {
                convo_id: ConvoId("c1".to_owned()),
                content: "draft to restore".to_owned(),
            })
            .unwrap();

        match next(&mut stream).await {
            Update::ActionFailed(ActionError::SendFailed {
                convo_id,
                content,
                reason,
            }) => {
                assert_eq!(convo_id.as_str(), "c1");
                assert_eq!(content, "draft to restore");
                assert!(reason.contains("mls exploded"), "reason: {reason}");
            }
            other => panic!("expected SendFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn events_decode_filter_and_survive_malformed_payloads() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "initialising", &[]);

        let mut stream = Box::pin(run(config(), single(transport)));
        let _control = start_initialising(&mut stream).await;

        controller.emit("module_event", "this is not json");
        controller.emit(
            "module_event",
            r#"["other_module","message_received","c0","nope",1,"x"]"#,
        );
        controller.emit(
            "module_event",
            r#"["chat_module","message_received","only-convo"]"#,
        );
        controller.emit(
            "module_event",
            r#"["chat_module","message_received","c1","hi",123,"peer"]"#,
        );

        match next(&mut stream).await {
            Update::Event(ChatEvent::MessageReceived {
                convo_id,
                content,
                timestamp_ms,
                sender,
            }) => {
                assert_eq!(convo_id.as_str(), "c1");
                assert_eq!(content, "hi");
                assert_eq!(timestamp_ms, 123);
                assert_eq!(sender, "peer");
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delivery_event_forwards_raw_event_then_phase_then_resync() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "initialising", &[]);

        let mut stream = Box::pin(run(config(), single(transport)));
        let _control = start_initialising(&mut stream).await;

        controller.emit(
            "module_event",
            r#"["chat_module","delivery_state_changed","online",""]"#,
        );

        match next(&mut stream).await {
            Update::Event(ChatEvent::DeliveryStateChanged {
                state: DeliveryState::Online,
                ..
            }) => {}
            other => panic!("expected the raw event first, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::Phase(Phase::Online) => {}
            other => panic!("expected Phase(Online), got {other:?}"),
        }
        match next(&mut stream).await {
            Update::Ready { .. } => {}
            other => panic!("expected Ready, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::ConversationsSnapshot(_) => {}
            other => panic!("expected the resync snapshot, got {other:?}"),
        }

        // Error and stopped states map to their phases (after the raw
        // event), and an unknown state is ignored.
        controller.emit(
            "module_event",
            r#"["chat_module","delivery_state_changed","error","boom"]"#,
        );
        match next(&mut stream).await {
            Update::Event(ChatEvent::DeliveryStateChanged { .. }) => {}
            other => panic!("expected the raw event, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::Phase(Phase::DeliveryError { detail }) => {
                assert_eq!(detail, "boom");
            }
            other => panic!("expected DeliveryError, got {other:?}"),
        }

        controller.emit(
            "module_event",
            r#"["chat_module","delivery_state_changed","weird",""]"#,
        );
        controller.emit(
            "module_event",
            r#"["chat_module","delivery_state_changed","stopped",""]"#,
        );
        match next(&mut stream).await {
            Update::Event(ChatEvent::DeliveryStateChanged {
                state: DeliveryState::Unknown,
                ..
            }) => {}
            other => panic!("expected the unknown-state event, got {other:?}"),
        }
        // No phase for the unknown state: the next update is the stopped
        // event.
        match next(&mut stream).await {
            Update::Event(ChatEvent::DeliveryStateChanged {
                state: DeliveryState::Stopped,
                ..
            }) => {}
            other => panic!("expected the stopped event, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::Phase(Phase::DeliveryStopped) => {}
            other => panic!("expected DeliveryStopped, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn conversation_updated_triggers_a_resync() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "initialising", &[]);

        let mut stream = Box::pin(run(config(), single(transport)));
        let _control = start_initialising(&mut stream).await;

        controller.emit(
            "module_event",
            r#"["chat_module","conversation_updated","c1"]"#,
        );

        match next(&mut stream).await {
            Update::Event(ChatEvent::ConversationUpdated { convo_id }) => {
                assert_eq!(convo_id.as_str(), "c1");
            }
            other => panic!("expected ConversationUpdated, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::Ready { .. } => {}
            other => panic!("expected the refetch Ready, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::ConversationsSnapshot(_) => {}
            other => panic!("expected the refetch snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn members_changed_triggers_a_member_reload() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "initialising", &[]);

        let mut stream = Box::pin(run(config(), single(transport)));
        let _control = start_initialising(&mut stream).await;

        controller
            .emit("module_event", r#"["chat_module","members_changed","g1"]"#);

        match next(&mut stream).await {
            Update::Event(ChatEvent::MembersChanged { convo_id }) => {
                assert_eq!(convo_id.as_str(), "g1");
            }
            other => panic!("expected MembersChanged, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::MembersLoaded { convo_id, .. } => {
                assert_eq!(convo_id.as_str(), "g1");
            }
            other => panic!("expected MembersLoaded, got {other:?}"),
        }

        let member_calls = controller
            .calls()
            .iter()
            .filter(|(method, args)| {
                method == "callModuleMethod"
                    && args.contains(r#""list_group_members""#)
            })
            .count();
        assert_eq!(member_calls, 1);
    }

    #[tokio::test]
    async fn events_flow_while_an_action_is_in_flight() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "online", &[]);

        let mut stream = Box::pin(run(config(), single(transport)));
        let mut control = start_online(&mut stream).await;

        // send_message emits a wire event mid-call, then blocks until
        // released: the event must reach the stream while the send is
        // still in flight.
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        {
            let emitter = controller.clone();
            controller.respond_with("callModuleMethod", move |args| {
                let args: Value =
                    serde_json::from_str(args).expect("call args are JSON");
                let method = args[1].as_str().expect("method name").to_owned();
                if method == "send_message" {
                    emitter.emit(
                        "module_event",
                        r#"["chat_module","message_received","c9","ping",42,"peer"]"#,
                    );
                    release_rx.lock().unwrap().recv().unwrap();
                }
                Ok(json!({
                    "status": "ok",
                    "module": "chat_module",
                    "method": method,
                    "result": default_result(&method, "online"),
                })
                .to_string())
            });
        }

        control
            .try_send(Control::SendMessage {
                convo_id: ConvoId("c1".to_owned()),
                content: "hello".to_owned(),
            })
            .unwrap();

        match next(&mut stream).await {
            Update::Event(ChatEvent::MessageReceived { convo_id, .. }) => {
                assert_eq!(convo_id.as_str(), "c9");
            }
            other => panic!("expected the in-flight event, got {other:?}"),
        }

        release_tx.send(()).unwrap();

        // The released send completes; a follow-up load round-trips,
        // proving the loop serves controls again.
        control
            .try_send(Control::LoadMessages {
                convo_id: ConvoId("c2".to_owned()),
            })
            .unwrap();
        match next(&mut stream).await {
            Update::MessagesLoaded { convo_id, .. } => {
                assert_eq!(convo_id.as_str(), "c2");
            }
            other => panic!("expected MessagesLoaded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn installation_name_is_applied_and_carried_by_ready() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(
            &controller,
            "online",
            &[("get_installation_name", json!("laptop"))],
        );
        let mut config = config();
        config.installation_name = Some("laptop".to_owned());

        let mut stream = Box::pin(run(config, single(transport)));
        let _control = controller_of(&mut stream).await;

        collect_until(&mut stream, |update| {
            matches!(update, Update::ConversationsSnapshot(_))
        })
        .await;
        match next(&mut stream).await {
            Update::Phase(Phase::Online) => {}
            other => panic!("expected Phase(Online), got {other:?}"),
        }
        match next(&mut stream).await {
            Update::Ready {
                my_address,
                installation_name,
            } => {
                assert_eq!(my_address, "my-address");
                assert_eq!(installation_name.as_deref(), Some("laptop"));
            }
            other => panic!("expected Ready, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::ConversationsSnapshot(_) => {}
            other => panic!("expected the resync snapshot, got {other:?}"),
        }

        let calls = controller.calls();
        let init_at = calls
            .iter()
            .position(|(method, args)| {
                method == "callModuleMethod" && args.contains(r#""init""#)
            })
            .expect("init was called");
        let set_at = calls
            .iter()
            .position(|(method, args)| {
                method == "callModuleMethod"
                    && args.contains(r#""set_installation_name""#)
            })
            .expect("set_installation_name was called");
        assert!(calls[set_at].1.contains(r#"["laptop"]"#));
        assert!(init_at < set_at, "the name is set after init");
    }

    #[tokio::test]
    async fn outer_timeout_restarts_with_a_fresh_transport() {
        let controllers: Arc<Mutex<Vec<FakeController>>> =
            Arc::new(Mutex::new(Vec::new()));
        let factory = {
            let controllers = controllers.clone();
            Box::new(move || {
                let (transport, controller) = FakeTransport::new();
                script_ladder(&controller, "initialising", &[]);
                controllers.lock().unwrap().push(controller);
                transport
            })
        };

        let mut stream = Box::pin(run(config(), Driver::Fake(factory)));
        let mut control = start_initialising(&mut stream).await;

        controllers.lock().unwrap()[0].respond_once(
            "callModuleMethod",
            Err(IpcError::Timeout {
                method: "callModuleMethod".to_owned(),
            }),
        );
        control
            .try_send(Control::LoadMessages {
                convo_id: ConvoId("c1".to_owned()),
            })
            .unwrap();

        match next(&mut stream).await {
            Update::ActionFailed(ActionError::LoadFailed { what, .. }) => {
                assert_eq!(what, LoadKind::Messages);
            }
            other => panic!("expected LoadFailed, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::Phase(Phase::Restarting { attempt: 1 }) => {}
            other => panic!("expected Restarting, got {other:?}"),
        }

        // The rebuilt stack serves calls again.
        collect_until(&mut stream, |update| {
            matches!(update, Update::ConversationsSnapshot(_))
        })
        .await;
        match next(&mut stream).await {
            Update::Phase(Phase::InitialisingChat) => {}
            other => panic!("expected the seeded phase, got {other:?}"),
        }
        assert_eq!(controllers.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn dead_link_restarts_with_a_fresh_transport() {
        let controllers: Arc<Mutex<Vec<FakeController>>> =
            Arc::new(Mutex::new(Vec::new()));
        let factory = {
            let controllers = controllers.clone();
            Box::new(move || {
                let (transport, controller) = FakeTransport::new();
                script_ladder(&controller, "initialising", &[]);
                controllers.lock().unwrap().push(controller);
                transport
            })
        };

        let mut stream = Box::pin(run(config(), Driver::Fake(factory)));
        let mut control = start_initialising(&mut stream).await;

        let first = controllers.lock().unwrap()[0].clone();
        first.respond_once("callModuleMethod", Ok("null".to_owned()));
        control
            .try_send(Control::LoadMessages {
                convo_id: ConvoId("c1".to_owned()),
            })
            .unwrap();

        match next(&mut stream).await {
            Update::ActionFailed(ActionError::LoadFailed { what, .. }) => {
                assert_eq!(what, LoadKind::Messages);
            }
            other => panic!("expected LoadFailed, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::Phase(Phase::Restarting { attempt: 1 }) => {}
            other => panic!("expected Restarting, got {other:?}"),
        }
        assert!(
            !first.connected(),
            "the dead transport must be torn down before the restart"
        );

        let reladder = collect_until(&mut stream, |update| {
            matches!(update, Update::ConversationsSnapshot(_))
        })
        .await;
        assert_eq!(
            phases(&reladder),
            vec![
                &Phase::StartingDaemon,
                &Phase::Connecting,
                &Phase::LoadingModule,
                &Phase::InitialisingChat,
            ]
        );
        match next(&mut stream).await {
            Update::Phase(Phase::InitialisingChat) => {}
            other => panic!("expected the seeded phase, got {other:?}"),
        }

        {
            let controllers = controllers.lock().unwrap();
            assert_eq!(controllers.len(), 2);
            let watches = controllers[1]
                .calls()
                .iter()
                .filter(|(method, _)| method == "watchModuleEvents")
                .count();
            assert_eq!(watches, 7, "a new daemon run watches again");
        }

        control
            .try_send(Control::LoadMessages {
                convo_id: ConvoId("c2".to_owned()),
            })
            .unwrap();
        match next(&mut stream).await {
            Update::MessagesLoaded { convo_id, .. } => {
                assert_eq!(convo_id.as_str(), "c2");
            }
            other => panic!("expected MessagesLoaded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn restart_exhaustion_surfaces_fatal() {
        let controllers: Arc<Mutex<Vec<FakeController>>> =
            Arc::new(Mutex::new(Vec::new()));
        let factory = {
            let controllers = controllers.clone();
            Box::new(move || {
                let (transport, controller) = FakeTransport::new();
                // Only the first run succeeds; later transports answer
                // every call with the default "null".
                if controllers.lock().unwrap().is_empty() {
                    script_ladder(&controller, "initialising", &[]);
                }
                controllers.lock().unwrap().push(controller);
                transport
            })
        };
        let mut config = config();
        config.restart_max_attempts = 2;

        let mut stream = Box::pin(run(config, Driver::Fake(factory)));
        let mut control = start_initialising(&mut stream).await;

        controllers.lock().unwrap()[0]
            .respond_once("callModuleMethod", Ok("null".to_owned()));
        control.try_send(Control::Resync).unwrap();

        let updates = collect_until(&mut stream, |update| {
            matches!(update, Update::Fatal(_))
        })
        .await;
        let restarts: Vec<u32> = updates
            .iter()
            .filter_map(|update| match update {
                Update::Phase(Phase::Restarting { attempt }) => Some(*attempt),
                _ => None,
            })
            .collect();
        assert_eq!(restarts, vec![1, 2]);
        match &updates[updates.len() - 2] {
            Update::Phase(Phase::Failed) => {}
            other => panic!("expected Phase(Failed), got {other:?}"),
        }
        match updates.last().unwrap() {
            Update::Fatal(BackendError::RestartExhausted {
                attempts: 2,
                ..
            }) => {}
            other => panic!("expected RestartExhausted, got {other:?}"),
        }
        assert_eq!(controllers.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn quit_orders_chat_shutdown_daemon_shutdown_then_stopped() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "online", &[]);

        let mut stream = Box::pin(run(config(), single(transport)));
        let mut control = start_online(&mut stream).await;

        control.try_send(Control::Quit).unwrap();

        match next(&mut stream).await {
            Update::Phase(Phase::ShuttingDown) => {}
            other => panic!("expected ShuttingDown, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::Stopped => {}
            other => panic!("expected Stopped, got {other:?}"),
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(50), stream.next())
                .await
                .is_err(),
            "nothing may follow Stopped"
        );

        let calls = controller.calls();
        let chat_shutdown = calls
            .iter()
            .position(|(method, args)| {
                method == "callModuleMethod" && args.contains(r#""shutdown""#)
            })
            .expect("chat_module.shutdown was called");
        let core_shutdown = calls
            .iter()
            .position(|(method, _)| method == "shutdown")
            .expect("core_service.shutdown was called");
        assert!(chat_shutdown < core_shutdown);
        assert_eq!(core_shutdown, calls.len() - 1);
        assert!(
            !controller.connected(),
            "the transport is disconnected before Stopped"
        );
    }

    #[tokio::test]
    async fn resync_controls_coalesce() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "online", &[]);

        let mut stream = Box::pin(run(config(), single(transport)));
        let mut control = start_online(&mut stream).await;

        control.try_send(Control::Resync).unwrap();
        control.try_send(Control::Resync).unwrap();
        control.try_send(Control::Resync).unwrap();

        match next(&mut stream).await {
            Update::Ready { .. } => {}
            other => panic!("expected Ready, got {other:?}"),
        }
        match next(&mut stream).await {
            Update::ConversationsSnapshot(_) => {}
            other => panic!("expected the resync snapshot, got {other:?}"),
        }

        // A follow-up roundtrip proves no further resync is queued.
        control
            .try_send(Control::LoadMessages {
                convo_id: ConvoId("c1".to_owned()),
            })
            .unwrap();
        match next(&mut stream).await {
            Update::MessagesLoaded { .. } => {}
            other => panic!("expected MessagesLoaded, got {other:?}"),
        }

        let address_calls = controller
            .calls()
            .iter()
            .filter(|(method, args)| {
                method == "callModuleMethod"
                    && args.contains(r#""get_address""#)
            })
            .count();
        // One from the fresh-online seed, one for the three coalesced
        // manual resyncs.
        assert_eq!(address_calls, 2);
    }
}
