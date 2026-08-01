//! Fixture builders: the "previous run" on disk, and the scripted backend
//! updates a run replays. Everything here is a one-liner a case can copy.

use data::dashboard::BufferSettings;
use data::pane::Axis;
use data::{Buffer, Dashboard, Pane};

use crate::stream::{
    ActionError, ChatEvent, Conversation, ConvoId, Kind,
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
    vec![
        Update::Phase(Phase::StartingDaemon),
        Update::Phase(Phase::Connecting),
        Update::Phase(Phase::LoadingModule),
        Update::Phase(Phase::InitialisingChat),
        Update::Phase(Phase::Online),
        Update::Ready {
            my_address: "0xtestaddress".to_owned(),
            installation_name: Some("testkit".to_owned()),
        },
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
