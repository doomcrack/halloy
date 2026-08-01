//! The backend session state machine — the analogue of halloy's
//! `data::stream::run`. One session per app run; the returned stream's
//! FIRST item is always `Update::Controller` carrying the control sender.
//!
//! Startup ladder (the QML ChatBackend ordering, transposed):
//! daemon up → abi check → logos thread + `module_event` subscription →
//! `get_status` link check → `loadModule("chat_module")` →
//! `watchModuleEvents` per event (×7 explicit, or one wildcard behind
//! config) → `chat.init({delivery_preset})` → `list_conversations`
//! snapshot → `chat.status()` seeded through the SAME handler as the live
//! event so an already-online module triggers the resync path instead of
//! being swallowed.
//!
//! The module monitor is *not* on that ladder. It runs entirely from the
//! main loop's poll, so nothing a module does can delay chat's login: the
//! logos thread serializes every invoke, so an introspection call issued
//! before `chat.init` is a call `chat.init` waits behind.
//!
//! Main loop rules:
//! - Control channel (bounded, cap 32) is polled only when no *chat* action
//!   is in flight — natural serialization for the single-dispatch module.
//!   Events are always polled (they keep flowing during a slow 20s send).
//! - `conversation_updated` marks a resync and `members_changed` queues a
//!   member reload for its conversation — the events carry only the id, so
//!   the session refetches and the results arrive as
//!   `ConversationsSnapshot`/`MembersLoaded` (QML rehydrate parity).
//! - Mutating actions are gated on Online → `ActionFailed(NotOnline)`.
//! - A 5s health tick probes the daemon at the PROCESS level (never an lp
//!   call) so detection works while the logos thread is blocked.
//! - A 5s module-status poll runs `listModules` (and the introspection +
//!   watch registration of any module that has newly become loaded) in a
//!   slot of its own. It is dispatched only while no chat action is in
//!   flight, so it never queues ahead of the user's work, and it occupies
//!   nothing the control channel is gated on, so a wedged daemon cannot
//!   stop the user's next action from dispatching. Every failure is
//!   swallowed — a monitor refresh must never restart the session. Module
//!   events other than chat's are counted per poll window, not forwarded.
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

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;
use futures::stream::{self, Stream, StreamExt};
use logos_client::{
    FakeTransport, Gateway, GatewayError, IpcError, LogosHandle, ModuleStatus,
    RawEvent, Transport, decode_module_event,
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
/// Module-status poll period.
///
/// A module can abort at any time and the daemon then reports it as
/// `not_loaded` on the very next poll — there is no crash event to
/// subscribe to, so the poll *is* the detection mechanism and its period is
/// the worst-case delay before a dead module stops looking healthy. Five
/// seconds keeps that inside the window a person reads as "immediately",
/// and matches [`HEALTH_TICK`] so the two liveness signals never disagree
/// for long.
///
/// It is affordable at that rate because `listModules` never leaves the
/// daemon: it answers from the module runtime's own table and calls into no
/// module, so a poll cannot be delayed by (or delay) a busy module. The one
/// call that does leave — `getModuleInfo`, for the event contract — is made
/// once per module per daemon run, not once per poll.
///
/// It is also the delay before the sidebar first says anything: the ladder
/// asks the daemon nothing about modules, so the first report is the first
/// tick.
const MODULE_POLL_TICK: Duration = Duration::from_secs(5);
/// The daemon's word for a running module, the one status value that
/// decides whether it is worth asking a module about its contract.
const LOADED: &str = "loaded";
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

/// One tracked module, as the monitor sees it: what the daemon last said
/// about it, plus how busy its event stream was over the last poll window.
///
/// Every field is one the UI renders, and the whole struct is the
/// republish decision (see [`Session::publish_modules`]) — so a field
/// nothing reads is not merely dead weight here, it is a guarantee that the
/// sidebar repaints forever. `uptime_seconds` was exactly that: the daemon
/// grows it every second, no view ever showed it, and carrying it made the
/// "publish only what moved" check true on every single poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleState {
    pub name: String,
    /// The daemon's verbatim vocabulary (`loaded` / `not_loaded`), left
    /// unmapped on purpose — the domain layer owns the interpretation, and
    /// a value the daemon grows later must reach it unflattened.
    pub status: String,
    pub version: Option<String>,
    /// Module events seen in the window since the last report, and reset by
    /// it. A *rate sample*, not a stream: a syncing blockchain node emitted
    /// 1158 `newBlock` events in a few minutes of catch-up, so forwarding
    /// each one would thrash the UI, and a running total would only ever
    /// grow. Sampled per poll it is the "there is progress" pulse
    /// `logos-modules.md` §2b asks for, and it settles back to zero — once —
    /// when the module goes quiet.
    pub recent_events: u64,
}

impl ModuleState {
    /// A module we staged but have not heard about yet. `not_loaded` is the
    /// honest pre-poll guess: it is what the daemon reports for anything
    /// installed and idle, which every module except chat's closure is.
    fn seed(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            status: "not_loaded".to_owned(),
            version: None,
            recent_events: 0,
        }
    }
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
    /// The whole tracked module set, re-sent whenever any of it moved.
    /// Never sent when nothing changed, so a 5s poll on a quiet daemon puts
    /// nothing on the stream at all — and never sent by the ladder, which
    /// asks the daemon nothing about modules. The first one arrives with
    /// the first poll, a few seconds after login.
    Modules(Vec<ModuleState>),
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
    /// Manual module-status refresh, off the poll's schedule. Coalesced the
    /// same way a resync is: several requests before the next dispatch cost
    /// one `listModules`.
    RefreshModules,
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
    let modules: Vec<ModuleState> = config
        .modules
        .iter()
        .map(|name| ModuleState::seed(name))
        .collect();
    let mut session = Session {
        published: modules.clone(),
        modules,
        config,
        updates,
        delivery: DeliveryState::Initialising,
        initial_snapshot_done: false,
        resync_dirty: false,
        modules_dirty: false,
        pending_member_loads: Vec::new(),
        watched: HashSet::new(),
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

/// A boxed in-flight chat action. It emits its own updates; `Err` carries a
/// dead-link reason that aborts the run into the restart ladder.
type ActionFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// A boxed monitor pass. Separate from [`ActionFuture`] because it is not
/// chat work: it emits nothing itself, it cannot end the run, and it is
/// deliberately not what the control channel waits on.
type MonitorFuture = Pin<Box<dyn Future<Output = MonitorPass> + Send>>;

enum LoopExit {
    Quit,
    Dead(String),
}

enum Applied {
    Handled,
    Quit,
}

/// What one monitor pass learned, for the loop to fold in — only the loop
/// holds `&mut Session`.
#[derive(Default)]
struct MonitorPass {
    /// The rows the daemon reported, empty when the poll failed. An empty
    /// report folds to nothing, which is the right answer for both: the
    /// last known status stays on screen.
    report: Vec<ModuleStatus>,
    /// Modules this pass attempted to watch, whether or not the attempt
    /// succeeded — see [`Session::watched`] for why a failure is not
    /// retried.
    watched: Vec<String>,
}

enum Arm {
    Raw(Option<RawEvent>),
    Action(Result<(), String>),
    Monitor(MonitorPass),
    Control(Option<Control>),
    Health,
    ModulePoll,
}

/// Session-wide mutable state plus the update sender.
struct Session {
    config: BackendConfig,
    updates: mpsc::UnboundedSender<Update>,
    delivery: DeliveryState,
    initial_snapshot_done: bool,
    resync_dirty: bool,
    modules_dirty: bool,
    pending_member_loads: Vec<ConvoId>,
    /// The tracked module set, in catalogue order with anything the daemon
    /// reported but we did not stage appended.
    modules: Vec<ModuleState>,
    /// The last set actually sent as [`Update::Modules`]. Publishing is
    /// decided against this rather than against the state before a fold,
    /// so an event pulse that moved between two identical status reports
    /// still reaches the UI, and a poll that changed nothing stays silent.
    published: Vec<ModuleState>,
    /// Modules whose event watches this daemon run has already registered —
    /// or tried to. There is no unwatch, so a second registration would
    /// duplicate every forwarder; and a module whose introspection times out
    /// must not cost that timeout again on every poll. One attempt per
    /// module per run, made the first time the daemon reports it loaded.
    watched: HashSet<String>,
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

    /// Index of `module`'s row, creating one if this is a name we have not
    /// seen. Rows are created rather than dropped because attribution is by
    /// tag string, not by a known-module list: a module that renames itself
    /// mid-flight should become visible, not silent.
    fn module_row(&mut self, module: &str) -> usize {
        match self.modules.iter().position(|state| state.name == module) {
            Some(index) => index,
            None => {
                self.modules.push(ModuleState::seed(module));
                self.modules.len() - 1
            }
        }
    }

    /// Records one event from a module that is not chat. Every event name
    /// counts, modelled or not — this *is* the fallback for an event we do
    /// not know, and it is why an unrecognised event can never be fatal.
    /// The name itself is only logged: what the pulse says is "this module
    /// is doing something", and a name would need a whole vocabulary to
    /// mean more than that.
    ///
    /// Deliberately emits nothing. The count rides out with the next status
    /// publish, which coalesces a flood into one repaint.
    fn note_module_event(&mut self, module: &str, event: &str) {
        log::trace!("{module} emitted {event}");
        let index = self.module_row(module);
        let state = &mut self.modules[index];
        state.recent_events = state.recent_events.saturating_add(1);
    }

    /// Folds one `listModules` report into the tracked set.
    ///
    /// A tracked module the report does not mention keeps its current row.
    /// The daemon lists every *installed* module whatever its load state,
    /// so silence means "not installed on this machine", which this
    /// read-only increment has no action to offer for and renders the same
    /// as idle.
    fn apply_modules(&mut self, report: Vec<ModuleStatus>) {
        for row in report {
            let index = self.module_row(&row.name);
            let state = &mut self.modules[index];
            state.status = row.status;
            state.version = row.version;
        }

        self.publish_modules();
    }

    /// Drops back to the staged set. A restart means a fresh daemon, so
    /// every module is idle again until the new one says otherwise, the
    /// event counters belong to a process that is gone, and the watches
    /// were registered against forwarders that died with it.
    fn reset_modules(&mut self) {
        self.modules = self
            .config
            .modules
            .iter()
            .map(|name| ModuleState::seed(name))
            .collect();
        self.watched.clear();

        self.publish_modules();
    }

    /// Sends the tracked set if it differs from what the UI last saw, then
    /// closes the event window the report just described.
    ///
    /// Nothing to say is said with silence: on a quiet daemon the 5s poll
    /// puts no updates on the stream at all. That holds only because every
    /// field compared here is one the UI renders — one field that moves on
    /// its own (a running total, an uptime the daemon grows every second)
    /// makes the comparison differ forever, and the coalescing this exists
    /// for is gone with it.
    /// The window roll runs on every publish, published or not, so counts
    /// describe one poll interval each rather than accumulating; the last
    /// busy window is followed by exactly one report of zero, and then
    /// silence.
    fn publish_modules(&mut self) {
        if self.modules != self.published {
            self.published = self.modules.clone();
            self.emit(Update::Modules(self.published.clone()));
        }

        for state in &mut self.modules {
            state.recent_events = 0;
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
    session.modules_dirty = false;
    session.pending_member_loads.clear();
    session.reset_modules();

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

/// Registers one module's watches from the contract it reports, never from
/// a list written down here.
///
/// The blockchain module we ship exposes one event (`newBlock`) while its
/// master branch exposes three; a hardcoded list would therefore either
/// miss events or register watches that never fire, depending only on which
/// build happens to be staged. `module_info` is the sole authority, and it
/// answers with an empty contract for a module that is not loaded — which
/// is why only loaded modules are asked.
async fn watch_module(gateway: &Gateway, module: &str) {
    let events = match gateway.module_info(module).await {
        Ok(info) => info.events,
        Err(error) => {
            log::warn!(
                "getModuleInfo({module}) failed; not watching it: {error}"
            );
            return;
        }
    };

    if events.is_empty() {
        log::debug!("{module} reports no events; nothing to watch");
        return;
    }

    for event in &events {
        match gateway.watch_module_events(module, event).await {
            Ok(true) => {}
            Ok(false) => {
                log::warn!("daemon declined the {module}.{event} watch");
            }
            Err(error) => {
                log::warn!("watching {module}.{event} failed: {error}");
            }
        }
    }
}

async fn main_loop(
    session: &mut Session,
    stack: &mut Stack,
    control_rx: &mut mpsc::Receiver<Control>,
) -> LoopExit {
    let mut in_flight: Option<ActionFuture> = None;
    let mut monitor: Option<MonitorFuture> = None;
    let mut health = tokio::time::interval(HEALTH_TICK);
    // First tick a full period out, not immediately: the login burst
    // (init, snapshot, status seed, and the resync an already-online module
    // triggers) owns the logos thread for those seconds, and the monitor
    // queueing behind it would be the startup delay this poll exists to
    // avoid. The staged rows carry the sidebar until the first report.
    let mut module_poll = tokio::time::interval_at(
        tokio::time::Instant::now() + MODULE_POLL_TICK,
        MODULE_POLL_TICK,
    );

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

        // Observation goes last and never first: dispatched only while chat
        // has nothing in flight, so a poll can never be the call a user's
        // action queues behind on the logos thread. Once dispatched it runs
        // in its own slot, so the control channel keeps flowing under it.
        if monitor.is_none() && session.modules_dirty && in_flight.is_none() {
            session.modules_dirty = false;
            monitor =
                Some(monitor_pass(&stack.gateway, session.watched.clone()));
        }

        let arm = tokio::select! {
            raw = stack.events.recv() => Arm::Raw(raw),
            outcome = async {
                in_flight
                    .as_mut()
                    .expect("in-flight action gated by the branch condition")
                    .await
            }, if in_flight.is_some() => Arm::Action(outcome),
            pass = async {
                monitor
                    .as_mut()
                    .expect("monitor pass gated by the branch condition")
                    .await
            }, if monitor.is_some() => Arm::Monitor(pass),
            control = control_rx.next(), if in_flight.is_none() => {
                Arm::Control(control)
            }
            _ = health.tick(), if stack.supervisor.is_some() => Arm::Health,
            _ = module_poll.tick() => Arm::ModulePoll,
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
            Arm::Monitor(pass) => {
                monitor = None;
                session.watched.extend(pass.watched);
                session.apply_modules(pass.report);
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
            Arm::ModulePoll => session.modules_dirty = true,
        }
    }
}

/// Decodes one wire event and demuxes it by module: anything that is not
/// chat is counted against that module's monitor row and goes no further;
/// chat's own events take exactly the path they always did — forward the
/// raw `Update::Event`, then feed `delivery_state_changed` into the
/// delivery handler. Undecodable payloads are logged, never fatal.
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
        session.note_module_event(&module_event.module, &module_event.event);
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
        Control::RefreshModules => {
            session.modules_dirty = true;
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
        | Control::RefreshModules
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
            Control::Resync | Control::RefreshModules | Control::Quit => None,
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

/// One monitor pass: `listModules`, then the introspection and watch
/// registration of every module the daemon now reports loaded that this run
/// has not asked about yet.
///
/// The watches live here rather than on the startup ladder for two reasons.
/// The logos thread serializes every invoke, so an introspection call made
/// before `chat.init` is a call `chat.init` waits behind — a module stalling
/// to its 5s timeout would delay chat's login by that much. And a module
/// loaded *after* startup used to get no watches at all; asked once per
/// module rather than once per run, the monitor converges on its own.
///
/// Every failure is swallowed, including a dead link. Observation must
/// never be the thing that decides the chat session is dead — a daemon too
/// old to know `listModules` answers with the same literal `null` a dead
/// link does, and that would put a working session into the restart ladder
/// on the strength of a monitor refresh. Real link death still surfaces
/// through chat's own calls and the process-level health tick.
fn monitor_pass(gateway: &Gateway, watched: HashSet<String>) -> MonitorFuture {
    let gateway = gateway.clone();
    Box::pin(async move {
        let report = match gateway.list_modules().await {
            Ok(report) => report,
            Err(error) => {
                log::debug!(
                    "listModules failed; keeping the last known module \
                     status: {error}"
                );
                return MonitorPass::default();
            }
        };

        // Chat is excluded: its watches are registered from
        // `ChatEvent::NAMES` (or one wildcard) by the ladder, and there is
        // no unwatch — re-registering here would duplicate every chat
        // forwarder. An idle module is skipped because its contract is
        // empty until it loads.
        let fresh: Vec<String> = report
            .iter()
            .filter(|row| {
                row.name != MODULE
                    && row.status == LOADED
                    && !watched.contains(&row.name)
            })
            .map(|row| row.name.clone())
            .collect();

        for module in &fresh {
            watch_module(&gateway, module).await;
        }

        MonitorPass {
            report,
            watched: fresh,
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
        // Answered but never asked by the ladder: the monitor polls from
        // the main loop. Chat only, so a poll registers no extra watches
        // and makes no `getModuleInfo` call.
        controller.respond(
            "listModules",
            r#"[{"name":"chat_module","status":"loaded","version":"0.2.1"}]"#,
        );

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
        assert!(
            updates.iter().all(|update| modules_of(update).is_none()),
            "the ladder publishes no module status: the monitor has not \
             run yet, and saying so would be inventing one",
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
            // Nothing of the monitor's sits between chat's watches and
            // chat's init: every call the ladder makes is chat's own, in
            // its own order, with its own arguments.
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

    /// The staged set is the application's, not the daemon's, so it is
    /// supplied rather than discovered.
    fn config_with_modules(modules: &[&str]) -> BackendConfig {
        let mut config = config();
        config.modules =
            modules.iter().map(|name| (*name).to_owned()).collect();
        config
    }

    fn watches(controller: &FakeController) -> Vec<String> {
        controller
            .calls()
            .into_iter()
            .filter(|(method, _)| method == "watchModuleEvents")
            .map(|(_, args)| args)
            .collect()
    }

    fn modules_of(update: &Update) -> Option<&[ModuleState]> {
        match update {
            Update::Modules(modules) => Some(modules),
            _ => None,
        }
    }

    fn state<'a>(modules: &'a [ModuleState], name: &str) -> &'a ModuleState {
        modules
            .iter()
            .find(|state| state.name == name)
            .unwrap_or_else(|| panic!("no row for {name}"))
    }

    /// Announces every `listModules` call on the returned channel and
    /// answers it from `rows`, which the test may move between polls.
    ///
    /// Waiting for the *call* is what lets a test assert a negative. The
    /// thing under test is whether an update is emitted at all, so a test
    /// that can only wait for updates can only ever prove the half that
    /// emits one — which is exactly how a republish on every poll survived
    /// a test named for catching it.
    fn polling(
        controller: &FakeController,
        rows: &Arc<Mutex<String>>,
    ) -> tokio_mpsc::UnboundedReceiver<()> {
        let (called, calls) = tokio_mpsc::unbounded_channel();
        let rows = rows.clone();
        controller.respond_with("listModules", move |_| {
            // Read first, announce second: the test flips `rows` the moment
            // it hears about a call, and this answer is already decided.
            let answer = rows.lock().unwrap().clone();
            let _ = called.send(());
            Ok(answer)
        });
        calls
    }

    /// One `blockchain_module` row, with the uptime a live daemon grows
    /// every second.
    fn row(status: &str, uptime: u32) -> String {
        format!(
            r#"[{{"name":"blockchain_module","status":"{status}",
                  "version":"0.2.0","uptime_seconds":{uptime}}}]"#
        )
    }

    /// Refreshes, and returns once the daemon has been *asked* — carrying
    /// every update the session emitted in the meantime.
    ///
    /// The updates are returned rather than dropped because they are the
    /// evidence: the fold of the previous poll lands in this window, so a
    /// poll that was supposed to stay silent is caught by what this hands
    /// back. The session only advances while its stream is polled, hence
    /// the pump; `biased` closes the window on the call itself rather than
    /// draining on past it, so an update this returns belongs to a poll
    /// that already happened. Anything it leaves behind is still on the
    /// stream for the caller's own `next` to trip over.
    async fn poll(
        stream: &mut (impl Stream<Item = Update> + Unpin),
        control: &mut mpsc::Sender<Control>,
        calls: &mut tokio_mpsc::UnboundedReceiver<()>,
    ) -> Vec<Update> {
        control.try_send(Control::RefreshModules).unwrap();

        let mut meanwhile = Vec::new();
        let pump = async {
            loop {
                tokio::select! {
                    biased;
                    called = calls.recv() => {
                        called.expect("the poll reached the daemon");
                        return;
                    }
                    update = stream.next() => {
                        meanwhile.push(update.expect("update stream ended"));
                    }
                }
            }
        };
        tokio::time::timeout(NEXT_TIMEOUT, pump)
            .await
            .expect("timed out waiting for the poll");

        meanwhile
    }

    fn republished(updates: &[Update]) -> bool {
        updates.iter().any(|update| modules_of(update).is_some())
    }

    /// Event watches for other modules must come from the contract the
    /// module reports, never from a list written down here: the same
    /// blockchain module ships with one event on the tag we run and three
    /// on master, so a hardcoded list would either miss events or register
    /// watches that never fire depending on which build is staged.
    ///
    /// The same pass pins four things: an idle module is not asked for a
    /// contract it cannot have, a module that loads *later* is picked up by
    /// the poll that sees it (the watches are no longer a one-shot on the
    /// startup ladder), chat is never re-watched, and no module is asked
    /// twice — there is no unwatch, so a second registration would
    /// duplicate every forwarder.
    #[tokio::test]
    async fn module_watches_come_from_the_reported_contract() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "initialising", &[]);
        let rows = Arc::new(Mutex::new(
            r#"[{"name":"chat_module","status":"loaded","version":"0.2.1"},
                {"name":"blockchain_module","status":"not_loaded","version":"0.2.0"}]"#
                .to_owned(),
        ));
        let mut calls = polling(&controller, &rows);
        controller.respond(
            "getModuleInfo",
            r#"{"name":"blockchain_module","status":"loaded","version":"0.2.0",
                "dependencies":[],"dependents":[],
                "events":[{"name":"newBlock","type":"event"}]}"#,
        );

        let mut stream = Box::pin(run(config(), single(transport)));
        let mut control = start_initialising(&mut stream).await;

        // Two passes, because a pass is only provably finished once the
        // next one has been dispatched — the monitor holds a single slot.
        poll(&mut stream, &mut control, &mut calls).await;
        poll(&mut stream, &mut control, &mut calls).await;
        assert!(
            !controller
                .calls()
                .iter()
                .any(|(method, _)| method == "getModuleInfo"),
            "an idle module has no live contract to report",
        );

        *rows.lock().unwrap() =
            r#"[{"name":"chat_module","status":"loaded","version":"0.2.1"},
                {"name":"blockchain_module","status":"loaded","version":"0.2.0"}]"#
                .to_owned();
        poll(&mut stream, &mut control, &mut calls).await;
        // The report is folded in the same breath as the watches, so the
        // update is the signal that the pass is done.
        match next(&mut stream).await {
            Update::Modules(modules) => {
                assert_eq!(state(&modules, "blockchain_module").status, LOADED);
            }
            other => panic!("expected the loaded module, got {other:?}"),
        }

        // A third poll of an unchanged report must ask nothing again.
        poll(&mut stream, &mut control, &mut calls).await;
        poll(&mut stream, &mut control, &mut calls).await;

        let introspected: Vec<String> = controller
            .calls()
            .into_iter()
            .filter(|(method, _)| method == "getModuleInfo")
            .map(|(_, args)| args)
            .collect();
        assert_eq!(introspected, vec![r#"["blockchain_module"]"#.to_owned()]);

        let mut expected: Vec<String> = ChatEvent::NAMES
            .iter()
            .map(|name| format!(r#"["chat_module","{name}"]"#))
            .collect();
        expected.push(r#"["blockchain_module","newBlock"]"#.to_owned());
        assert_eq!(watches(&controller), expected);
    }

    /// A module that cannot be introspected must cost only its own watches,
    /// and must cost them once. `getModuleInfo` is the one call that leaves
    /// the daemon, so it is the one most likely to hang — and a poll that
    /// retried it would spend that timeout again every five seconds for the
    /// rest of the run.
    #[tokio::test]
    async fn a_failing_module_introspection_is_not_retried_every_poll() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "online", &[]);
        let rows = Arc::new(Mutex::new(
            r#"[{"name":"delivery_module","status":"loaded","version":"0.1.3"}]"#
                .to_owned(),
        ));
        let mut calls = polling(&controller, &rows);
        controller.respond_once(
            "getModuleInfo",
            Err(IpcError::Timeout {
                method: "getModuleInfo".to_owned(),
            }),
        );

        let mut stream = Box::pin(run(config(), single(transport)));
        let mut control = start_online(&mut stream).await;

        poll(&mut stream, &mut control, &mut calls).await;
        // The status half of the pass survives the introspection half.
        match next(&mut stream).await {
            Update::Modules(modules) => {
                assert_eq!(state(&modules, "delivery_module").status, LOADED);
            }
            other => panic!("expected the report anyway, got {other:?}"),
        }

        poll(&mut stream, &mut control, &mut calls).await;
        poll(&mut stream, &mut control, &mut calls).await;

        let introspections = controller
            .calls()
            .iter()
            .filter(|(method, _)| method == "getModuleInfo")
            .count();
        assert_eq!(introspections, 1, "the failure was retried");
        let expected: Vec<String> = ChatEvent::NAMES
            .iter()
            .map(|name| format!(r#"["chat_module","{name}"]"#))
            .collect();
        assert_eq!(watches(&controller), expected);
    }

    /// The monitor is off chat's critical path, and off the path of every
    /// action the user takes afterwards.
    ///
    /// `listModules` never answers here until the test lets it. The ladder
    /// has to reach its snapshot regardless: the logos thread serializes
    /// every invoke, so a monitor call issued before `chat.init` is a call
    /// `chat.init` waits behind, and a module stalling to its timeout would
    /// be a login stalling to its timeout. Once the poll IS in flight, the
    /// control channel has to keep flowing — a gated send is answered by
    /// the session alone, so an answer proves the control was dequeued
    /// while the poll had still not returned.
    #[tokio::test]
    async fn a_wedged_module_poll_blocks_neither_login_nor_the_next_action() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "initialising", &[]);
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let blocked = Mutex::new(blocked);
        let (called, mut calls) = tokio_mpsc::unbounded_channel();
        controller.respond_with("listModules", move |_| {
            let _ = called.send(());
            let _ = blocked.lock().unwrap().recv();
            Ok(r#"[{"name":"chat_module","status":"loaded"}]"#.to_owned())
        });

        let mut stream = Box::pin(run(config(), single(transport)));
        // Reaching the seeded phase at all is the assertion: with the
        // monitor back on the ladder this sits in `listModules` until the
        // test's own timeout.
        let mut control = start_initialising(&mut stream).await;

        poll(&mut stream, &mut control, &mut calls).await;

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
            other => panic!(
                "the control channel waited on the module poll: {other:?}"
            ),
        }

        release.send(()).unwrap();
        match next(&mut stream).await {
            Update::Modules(modules) => {
                assert_eq!(state(&modules, "chat_module").status, LOADED);
            }
            other => panic!("expected the released poll, got {other:?}"),
        }
    }

    /// The staged set gives every module a row before the daemon has said
    /// anything, and the first report fills those rows in rather than
    /// replacing the list — a module the daemon never mentions keeps its
    /// place instead of disappearing from the sidebar.
    #[tokio::test]
    async fn staged_modules_hold_their_row_when_the_daemon_reports() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "initialising", &[]);
        controller.respond(
            "listModules",
            r#"[{"name":"chat_module","status":"loaded","version":"0.2.1","uptime_seconds":7}]"#,
        );

        let mut stream = Box::pin(run(
            config_with_modules(&["chat_module", "blockchain_module"]),
            single(transport),
        ));
        let mut control = start_initialising(&mut stream).await;

        control.try_send(Control::RefreshModules).unwrap();
        let modules = match next(&mut stream).await {
            Update::Modules(modules) => modules,
            other => panic!("expected the first report, got {other:?}"),
        };

        assert_eq!(
            modules.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["chat_module", "blockchain_module"]
        );
        assert_eq!(state(&modules, "chat_module").status, LOADED);
        assert_eq!(
            state(&modules, "chat_module").version.as_deref(),
            Some("0.2.1")
        );
        assert_eq!(state(&modules, "blockchain_module").status, "not_loaded");
    }

    /// A refresh that learns nothing new must say nothing: the poll runs
    /// every 5s forever, and an update per tick would repaint the sidebar
    /// for state the UI already has.
    ///
    /// The middle poll is the one that matters, and it is the one a real
    /// daemon serves constantly: same module, same status, same version,
    /// and an uptime one second further along. Silence there is asserted by
    /// what arrives *next* — a spurious `loaded` republish would be the
    /// update this test reads as the crash it went looking for. Asserting
    /// it with a follow-up round trip instead would prove nothing at all:
    /// the poll dispatches at lowest priority, so a chat reply outruns a
    /// queued republish whether or not one exists.
    #[tokio::test]
    async fn module_status_is_republished_only_when_it_moves() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "online", &[]);
        let rows = Arc::new(Mutex::new(row(LOADED, 7)));
        let mut calls = polling(&controller, &rows);
        controller.respond(
            "getModuleInfo",
            r#"{"name":"blockchain_module","status":"loaded","events":[]}"#,
        );

        let mut stream = Box::pin(run(
            config_with_modules(&["blockchain_module"]),
            single(transport),
        ));
        let mut control = start_online(&mut stream).await;

        poll(&mut stream, &mut control, &mut calls).await;
        match next(&mut stream).await {
            Update::Modules(modules) => {
                assert_eq!(state(&modules, "blockchain_module").status, LOADED);
            }
            other => panic!("expected the first report, got {other:?}"),
        }

        // Nothing moved but the daemon's clock.
        *rows.lock().unwrap() = row(LOADED, 12);
        poll(&mut stream, &mut control, &mut calls).await;

        // The module aborts; the daemon reports it as `not_loaded` on the
        // very next poll, and that is the only way we ever learn of it.
        // Whatever the uptime-only poll had to say lands in this window.
        *rows.lock().unwrap() = row("not_loaded", 0);
        let quiet = poll(&mut stream, &mut control, &mut calls).await;
        assert!(
            !republished(&quiet),
            "an uptime-only report was republished: {quiet:?}",
        );

        match next(&mut stream).await {
            Update::Modules(modules) => {
                assert_eq!(
                    state(&modules, "blockchain_module").status,
                    "not_loaded",
                    "the uptime-only poll republished",
                );
            }
            other => panic!("expected the changed status, got {other:?}"),
        }
    }

    /// A module event that is not chat's must reach the module state
    /// instead of being dropped — and must not be forwarded one-for-one: a
    /// syncing blockchain node emitted 1158 `newBlock` events in a few
    /// minutes of catch-up. The poll samples the count, which is what makes
    /// it a rate rather than a running total, and one quiet window later it
    /// is back to zero and stays silent.
    #[tokio::test]
    async fn non_chat_events_are_sampled_per_poll_and_never_forwarded() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "online", &[]);
        let rows = Arc::new(Mutex::new(row(LOADED, 7)));
        let mut calls = polling(&controller, &rows);
        controller.respond(
            "getModuleInfo",
            r#"{"name":"blockchain_module","status":"loaded",
                "events":[{"name":"newBlock","type":"event"}]}"#,
        );

        let mut stream = Box::pin(run(
            config_with_modules(&["blockchain_module"]),
            single(transport),
        ));
        let mut control = start_online(&mut stream).await;

        controller
            .emit("module_event", r#"["blockchain_module","newBlock",1]"#);
        controller
            .emit("module_event", r#"["blockchain_module","newBlock",2]"#);
        // An event we model nowhere is recorded just the same; an unknown
        // event from an unknown module must never be fatal.
        controller.emit("module_event", r#"["other_module","surprise"]"#);

        poll(&mut stream, &mut control, &mut calls).await;
        match next(&mut stream).await {
            Update::Modules(modules) => {
                assert_eq!(
                    state(&modules, "blockchain_module").recent_events,
                    2
                );
                assert_eq!(state(&modules, "other_module").recent_events, 1);
            }
            other => panic!("expected the sampled window, got {other:?}"),
        }

        // A quiet window reports zero — once.
        poll(&mut stream, &mut control, &mut calls).await;
        match next(&mut stream).await {
            Update::Modules(modules) => {
                assert_eq!(
                    state(&modules, "blockchain_module").recent_events,
                    0
                );
            }
            other => panic!("expected the quiet window, got {other:?}"),
        }

        poll(&mut stream, &mut control, &mut calls).await;
        *rows.lock().unwrap() = row("not_loaded", 0);
        let quiet = poll(&mut stream, &mut control, &mut calls).await;
        assert!(
            !republished(&quiet),
            "a second quiet window republished a pulse of zero: {quiet:?}",
        );
        match next(&mut stream).await {
            Update::Modules(modules) => {
                assert_eq!(
                    state(&modules, "blockchain_module").status,
                    "not_loaded"
                );
            }
            other => panic!("expected the changed status, got {other:?}"),
        }
    }

    /// The poll is observation. A daemon that does not know `listModules`
    /// answers with the same literal `null` a dead link does, so treating a
    /// failed refresh as a dead link would put a perfectly healthy session
    /// into the restart ladder on the strength of a monitor call.
    #[tokio::test]
    async fn a_failing_module_poll_never_restarts_the_session() {
        let (transport, controller) = FakeTransport::new();
        script_ladder(&controller, "online", &[]);
        let mut stream = Box::pin(run(config(), single(transport)));
        let mut control = start_online(&mut stream).await;

        controller.respond_once("listModules", Err(IpcError::DeadLink));
        control.try_send(Control::RefreshModules).unwrap();
        control
            .try_send(Control::LoadMessages {
                convo_id: ConvoId("c1".to_owned()),
            })
            .unwrap();

        match next(&mut stream).await {
            Update::MessagesLoaded { .. } => {}
            other => panic!("expected the session to carry on, got {other:?}"),
        }
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
