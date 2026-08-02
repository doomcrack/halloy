//! Fixture builders: the "previous run" on disk, and the scripted backend
//! updates a run replays. Everything here is a one-liner a case can copy.

use data::dashboard::BufferSettings;
use data::pane::Axis;
use data::{Buffer, Dashboard, Pane};

use crate::stream::{
    ActionError, ChatEvent, Conversation, ConvoId, DeliveryState, Kind,
    Message as WireMessage, ModuleState, Phase, Update,
};

/// The daemon's own crash sequence, verbatim from a real session. The abort
/// is announced under envelope `[info]` — the module's own dialect has
/// nothing to say about it — which is exactly why the level filter must not
/// be what decides whether the app learns a module died.
pub const CRASH: &[&str] = &[
    "[2026-07-31 21:27:53.228] [info] [logos] [blockchain_module] panicked \
     at library/std/src/thread/local.rs:428:25:",
    "[2026-07-31 21:27:53.228] [critical] [logos] [blockchain_module] FATAL: \
     module 'blockchain_module' crashed (signal 6).",
];

/// A persisted pane showing a conversation.
pub fn pane(id: &str) -> Pane {
    Pane::Buffer {
        buffer: Buffer::Conversation(id.into()),
    }
}

/// A persisted pane showing one module's log.
pub fn module_pane(name: &str) -> Pane {
    Pane::Buffer {
        buffer: Buffer::Module(name.into()),
    }
}

pub fn split_v(a: Pane, b: Pane) -> Pane {
    Pane::Split {
        axis: Axis::Vertical,
        ratio: 0.5,
        a: Box::new(a),
        b: Box::new(b),
    }
}

/// The dashboard a previous run wrote: a layout plus the conversation that
/// was focused when it quit.
pub fn persisted(layout: Pane, focus: Option<&str>) -> Dashboard {
    Dashboard {
        pane: layout,
        popout_panes: vec![],
        buffer_settings: BufferSettings::default(),
        focus_buffer: focus.map(|id| Buffer::Conversation(id.into())),
        sidebar: data::dashboard::Sidebar::Visible,
        member_add_explained: false,
    }
}

/// Repoints a persisted dashboard's focus at a module, which `persisted`
/// cannot express — its focus argument is a conversation id.
pub fn focused_on_module(mut dashboard: Dashboard, name: &str) -> Dashboard {
    dashboard.focus_buffer = Some(Buffer::Module(name.into()));

    dashboard
}

/// Gives `buffer` the per-buffer settings a previous run would have saved
/// for it. What `Dashboard::load`'s prune sweep is aimed at, and the thing a
/// module key must survive.
pub fn with_settings(mut dashboard: Dashboard, buffer: Buffer) -> Dashboard {
    let _ = dashboard.buffer_settings.entry(&buffer, None);

    dashboard
}

/// Collapses the sidebar. Thread-level text assertions are otherwise
/// ambiguous: the sidebar carries its own copy of several strings (a
/// conversation with no preview also reads "No messages yet").
pub fn without_sidebar(mut dashboard: Dashboard) -> Dashboard {
    dashboard.sidebar = data::dashboard::Sidebar::Hidden;

    dashboard
}

/// The phase ladder plus `Ready`, i.e. everything between the controller
/// and the first snapshot.
pub fn startup() -> Vec<Update> {
    let mut updates = startup_ladder();

    updates.push(Update::Ready {
        my_address: "0xtestaddress".to_owned(),
        installation_name: Some("testkit".to_owned()),
    });

    updates
}

/// The supervisor noticing the backend died and building it again, exactly
/// as a real run emits it: the restart announcement, then the backend
/// reseeding its own module set (`reset_modules` publishes `not_loaded` for
/// everything it stages), then the ladder over again.
///
/// `staged` is what the reseeded report names — the modules the backend
/// tracks, whatever they were doing before — and `restored` is what the
/// daemon reports once it is up. A module missing from `restored` is one
/// that did not come back.
pub fn restart(staged: &[&str], restored: &[(&str, &str)]) -> Vec<Update> {
    let mut updates = vec![Update::Phase(Phase::Restarting { attempt: 1 })];

    updates.push(modules(
        &staged
            .iter()
            .map(|name| (*name, "not_loaded"))
            .collect::<Vec<_>>(),
    ));
    updates.extend(startup_ladder());
    updates.push(modules(restored));

    updates
}

/// The restart ladder running out of attempts: the backend tried, failed
/// every time, and gave up. Nothing follows it — there is no daemon left to
/// answer a poll, so whatever the rows say when this lands is what the user
/// is left looking at.
pub fn restart_exhausted(attempts: u32) -> Vec<Update> {
    (1..=attempts)
        .map(|attempt| Update::Phase(Phase::Restarting { attempt }))
        .chain(std::iter::once(Update::Phase(Phase::Failed)))
        .collect()
}

/// The phase ladder on its own — a (re)start with nothing else attached.
pub fn startup_ladder() -> Vec<Update> {
    vec![
        Update::Phase(Phase::StartingDaemon),
        Update::Phase(Phase::Connecting),
        Update::Phase(Phase::LoadingModule),
        Update::Phase(Phase::InitialisingChat),
        Update::Phase(Phase::Online),
    ]
}

/// Delivery dropping back to `initialising` under a backend that is up and
/// answering polls.
///
/// The shape that makes the phase ladder ambiguous, and it is an ordinary
/// wire event rather than a fault: `handle_delivery` re-emits
/// `Phase(InitialisingChat)` for **every** `delivery_state_changed`
/// carrying `initialising`, so the same phase the ladder climbs also
/// arrives mid-run with no restart behind it. The live event is forwarded
/// first and the phase follows, exactly as the session machine orders them.
pub fn delivery_blip() -> Vec<Update> {
    vec![
        Update::Event(ChatEvent::DeliveryStateChanged {
            state: DeliveryState::Initialising,
            detail: String::new(),
        }),
        Update::Phase(Phase::InitialisingChat),
    ]
}

/// Delivery coming back from a blip, in the same order.
pub fn delivery_recovered() -> Vec<Update> {
    vec![
        Update::Event(ChatEvent::DeliveryStateChanged {
            state: DeliveryState::Online,
            detail: String::new(),
        }),
        Update::Phase(Phase::Online),
    ]
}

/// The ladder stopped part-way, at the phase the app loads chat in. What a
/// module genuinely being loaded looks like from the UI's side.
pub fn loading_module() -> Vec<Update> {
    vec![
        Update::Phase(Phase::Restarting { attempt: 1 }),
        Update::Phase(Phase::StartingDaemon),
        Update::Phase(Phase::Connecting),
        Update::Phase(Phase::LoadingModule),
    ]
}

/// The snapshot that replaces all conversation state — the reconciliation
/// point.
pub fn snapshot(ids: &[&str]) -> Update {
    Update::ConversationsSnapshot(
        ids.iter().copied().map(conversation).collect(),
    )
}

pub fn conversation(id: &str) -> Conversation {
    Conversation {
        convo_id: ConvoId(id.to_owned()),
        nickname: None,
        message_count: 0,
        last_activity_ms: 1_700_000_000_000,
        kind: Kind::Direct,
        name: None,
        description: None,
        preview: None,
    }
}

/// The module answering a `LoadMessages`.
pub fn messages_loaded(id: &str, texts: &[&str]) -> Update {
    Update::MessagesLoaded {
        convo_id: ConvoId(id.to_owned()),
        messages: texts
            .iter()
            .enumerate()
            .map(|(index, text)| WireMessage {
                from_self: false,
                content: (*text).to_owned(),
                timestamp_ms: 1_700_000_000_000 + index as i64,
                sender: Some("0xpeer".to_owned()),
            })
            .collect(),
    }
}

/// A live inbound message push.
pub fn received(id: &str, text: &str) -> Update {
    Update::Event(ChatEvent::MessageReceived {
        convo_id: ConvoId(id.to_owned()),
        content: text.to_owned(),
        timestamp_ms: 1_700_000_100_000,
        sender: "0xpeer".to_owned(),
    })
}

/// Delivery falling over, e.g. the daemon dying under the app.
pub fn offline(detail: &str) -> Update {
    Update::Phase(Phase::DeliveryError {
        detail: detail.to_owned(),
    })
}

/// The module refusing a send, which hands the draft back to the composer.
pub fn send_failed(id: &str, content: &str, reason: &str) -> Update {
    Update::ActionFailed(ActionError::SendFailed {
        convo_id: ConvoId(id.to_owned()),
        content: content.to_owned(),
        reason: reason.to_owned(),
    })
}

/// One blockchain reading, as the monitor pass publishes it.
///
/// `mode` and `slot` are the two the panel must show moving: the word
/// alone reads as a hang for the twenty-odd minutes a cold start takes.
pub fn blockchain(sequence: u64, mode: &str, height: u64, slot: u64) -> Update {
    use logos_blockchain_client::{CryptarchiaInfo, Probe, Sample};

    Update::Blockchain(Sample::now(
        sequence,
        Probe::Ready(CryptarchiaInfo {
            mode: mode.to_owned(),
            height,
            slot,
            tip: "b11eda4c0f9b2854".to_owned(),
            lib: "efa86ac70717d040".to_owned(),
        }),
        // What the whole bootstrap looks like: `get_blocks` serves
        // finalised blocks and nothing is finalised yet.
        Probe::Pending("no blocks yet"),
        Probe::Unsupported("this build reports no peer count"),
    ))
}

/// One `listModules` report, as the backend publishes it. `status` is the
/// daemon's verbatim word — `loaded` or `not_loaded`; it never says
/// `crashed`, which is the whole reason the log has to.
pub fn modules(report: &[(&str, &str)]) -> Update {
    Update::Modules(
        report
            .iter()
            .map(|(name, status)| ModuleState {
                name: (*name).to_owned(),
                status: (*status).to_owned(),
                version: None,
                recent_events: 0,
            })
            .collect(),
    )
}

/// The same report, with the event pulse the backend sampled over the poll
/// window it closed — what a module emitting events looks like from the UI's
/// side.
pub fn modules_pulsing(report: &[(&str, &str, u64)]) -> Update {
    Update::Modules(
        report
            .iter()
            .map(|(name, status, recent_events)| ModuleState {
                name: (*name).to_owned(),
                status: (*status).to_owned(),
                version: None,
                recent_events: *recent_events,
            })
            .collect(),
    )
}

/// One record of the app's own log, as the logging subscription emits it.
pub fn log_record(level: data::log::Level, message: &str) -> data::log::Record {
    data::log::Record {
        timestamp: chrono::Utc::now(),
        level,
        message: message.to_owned(),
    }
}

/// One raw line of the daemon's combined log, parsed exactly as the tailer
/// parses it — envelope, dialect, attribution and all. Tests write the real
/// wire shape so a parser change cannot quietly pass them.
pub fn module_line(raw: &str) -> data::module::tail::Line {
    let record = data::module::log::parse(raw);

    data::module::tail::Line {
        module: record
            .module
            .clone()
            .unwrap_or_else(data::module::ModuleId::daemon),
        record,
    }
}
