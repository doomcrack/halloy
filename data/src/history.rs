use core::fmt;

use chrono::{DateTime, Utc};

pub use self::manager::{Manager, Resource};
pub use self::metadata::ReadMarker;
use crate::conversation::ConvoId;
use crate::module::ModuleId;
use crate::{Buffer, Message, buffer, message};

pub mod manager;
pub mod metadata;

/// Persistence seam: module-side persistence lands upstream; until it does,
/// histories live in memory only. When it lands, key files on
/// `seahash("convo:{id}")` — never revive the `fmt::Binary`-of-Server scheme.
pub mod persistence {}

pub(crate) const MAX_MESSAGES: usize = 10_000;

/// Scrollback for one module's log, deliberately a fifth of a conversation's.
///
/// A terminal log is a tail, not an archive: delivery alone emitted ~2000
/// lines in a few minutes of one observed session (`logos-modules.md` §4), so a
/// conversation-sized cap would hold minutes of one chatty module and cost a
/// megabyte per open pane to do it. Anything older than the cap is in the
/// daemon's log file, which is where a real search belongs.
pub(crate) const MAX_MESSAGES_MODULE: usize = 2_000;

const TRUNC_COUNT: usize = 500;

/// Eviction batch for a module log. Proportional to its smaller cap — a
/// 500-message drain would discard a quarter of the pane every few seconds
/// under a syncing node.
const TRUNC_COUNT_MODULE: usize = 100;

/// Messages that arrive within this window of an existing message are
/// candidates for deduplication (a `get_messages` snapshot racing live
/// events delivers the same message with slightly different timestamps).
const FUZZ_SECONDS: i64 = 1;

pub(crate) fn truncate_messages(kind: &Kind, messages: &mut Vec<Message>) {
    let (cap, batch) = match kind {
        Kind::Module(_) => (MAX_MESSAGES_MODULE, TRUNC_COUNT_MODULE),
        Kind::Conversation(_) | Kind::Logs => (MAX_MESSAGES, TRUNC_COUNT),
    };

    if messages.len() > cap {
        messages.drain(0..batch);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    Conversation(ConvoId),
    /// One module's log stream, keyed by the module's wire name.
    Module(ModuleId),
    Logs,
}

impl Kind {
    pub fn from_buffer(buffer: Buffer) -> Option<Self> {
        match buffer {
            Buffer::Conversation(convo_id) => {
                Some(Kind::Conversation(convo_id))
            }
            Buffer::Module(module_id) => Some(Kind::Module(module_id)),
            Buffer::Internal(buffer::Internal::Logs) => Some(Kind::Logs),
            Buffer::Internal(buffer::Internal::ConfigEditor) => None,
        }
    }

    pub fn convo_id(&self) -> Option<&ConvoId> {
        match self {
            Kind::Conversation(convo_id) => Some(convo_id),
            Kind::Module(_) | Kind::Logs => None,
        }
    }

    pub fn module_id(&self) -> Option<&ModuleId> {
        match self {
            Kind::Module(module_id) => Some(module_id),
            Kind::Conversation(_) | Kind::Logs => None,
        }
    }

    /// Whether a backend `get_messages` snapshot will ever arrive for this
    /// kind.
    ///
    /// Only conversations have one. Application logs and module logs are
    /// produced locally — nothing will ever call `load_full` for them — so a
    /// [`History::Partial`] would sit there forever holding its lines in
    /// `pending_messages`, where `history_view` cannot see them, and the pane
    /// would render empty for the rest of the session. Those kinds are
    /// therefore born [`History::Full`] and must never be demoted back.
    pub fn awaits_snapshot(&self) -> bool {
        match self {
            Kind::Conversation(_) => true,
            Kind::Module(_) | Kind::Logs => false,
        }
    }
}

impl From<ModuleId> for Kind {
    fn from(module_id: ModuleId) -> Self {
        Kind::Module(module_id)
    }
}

impl From<ConvoId> for Kind {
    fn from(convo_id: ConvoId) -> Self {
        Kind::Conversation(convo_id)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Conversation(convo_id) => {
                write!(f, "conversation {convo_id}")
            }
            Kind::Module(module_id) => write!(f, "module {module_id}"),
            Kind::Logs => write!(f, "logs"),
        }
    }
}

impl From<Kind> for Buffer {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Conversation(convo_id) => Buffer::Conversation(convo_id),
            Kind::Module(module_id) => Buffer::Module(module_id),
            Kind::Logs => Buffer::Internal(buffer::Internal::Logs),
        }
    }
}

#[derive(Debug)]
pub enum History {
    Partial {
        kind: Kind,
        /// Messages recorded before the module snapshot lands (locally
        /// generated status lines, or live pushes racing `get_messages`).
        /// `load_full` merges them into the loaded history — without this
        /// they would have nowhere to live and be lost.
        pending_messages: Vec<Message>, // Sorted by Message.server_time
        unread_count: usize,
        max_triggers_unread: Option<DateTime<Utc>>,
        read_marker: Option<ReadMarker>,
        show_in_sidebar: bool,
    },
    Full {
        kind: Kind,
        messages: Vec<Message>, // Sorted by Message.server_time
        unread_count: usize,
        read_marker: Option<ReadMarker>,
        display_read_marker: Option<ReadMarker>,
        cleared: bool,
    },
}

impl History {
    /// The state a freshly tracked history starts in.
    ///
    /// `Partial` is a promise that a `get_messages` snapshot is coming. For a
    /// kind that awaits no snapshot ([`Kind::awaits_snapshot`]) that promise
    /// is never kept, so it starts `Full` instead — otherwise every line it is handed
    /// lands in `pending_messages`, which `history_view` deliberately refuses
    /// to render, and the buffer stays blank forever.
    pub(crate) fn new(kind: Kind) -> Self {
        if kind.awaits_snapshot() {
            Self::partial(kind)
        } else {
            Self::full(kind)
        }
    }

    fn partial(kind: Kind) -> Self {
        Self::Partial {
            kind,
            pending_messages: vec![],
            unread_count: 0,
            max_triggers_unread: None,
            read_marker: None,
            show_in_sidebar: false,
        }
    }

    fn full(kind: Kind) -> Self {
        Self::Full {
            kind,
            messages: vec![],
            unread_count: 0,
            read_marker: None,
            display_read_marker: None,
            cleared: false,
        }
    }

    pub fn unread_count(&self) -> usize {
        match self {
            History::Partial { unread_count, .. }
            | History::Full { unread_count, .. } => *unread_count,
        }
    }

    fn mark_unread(&mut self) {
        match self {
            History::Partial {
                unread_count,
                show_in_sidebar,
                ..
            } => {
                *unread_count += 1;
                *show_in_sidebar = true;
            }
            History::Full { unread_count, .. } => {
                *unread_count += 1;
            }
        }
    }

    fn has_unread(&self) -> bool {
        if self.unread_count() > 0 {
            return true;
        }

        match self {
            History::Partial {
                max_triggers_unread,
                read_marker,
                ..
            } => {
                // Read marker is prior to last known message which triggers unread
                if let Some(read_marker) = read_marker {
                    max_triggers_unread
                        .is_some_and(|max| read_marker.date_time() < max)
                }
                // Default state == unread if theres messages that trigger indicator
                else {
                    max_triggers_unread.is_some()
                }
            }
            History::Full {
                messages,
                display_read_marker,
                ..
            } => {
                let latest = metadata::latest_triggers_unread(messages);

                if let Some(display_read_marker) = display_read_marker {
                    latest.is_some_and(|latest| {
                        display_read_marker.date_time() < latest
                    })
                } else {
                    latest.is_some()
                }
            }
        }
    }

    fn add_message(&mut self, message: Message) {
        let triggers_unread = message.triggers_unread();

        match self {
            History::Partial {
                kind,
                pending_messages,
                unread_count,
                max_triggers_unread,
                show_in_sidebar,
                ..
            } => {
                if matches!(message.direction, message::Direction::Sent)
                    || (triggers_unread
                        && Some(message.server_time) > *max_triggers_unread)
                {
                    *show_in_sidebar = true;
                }

                if triggers_unread {
                    *unread_count += 1;
                    *max_triggers_unread =
                        (*max_triggers_unread).max(Some(message.server_time));
                }

                insert_message(kind, pending_messages, message);

                truncate_messages(kind, pending_messages);
            }
            History::Full {
                kind,
                messages,
                unread_count,
                ..
            } => {
                if insert_message(kind, messages, message) && triggers_unread {
                    *unread_count += 1;
                }

                truncate_messages(kind, messages);
            }
        }
    }

    /// Collapses a closed history back to `Partial`, dropping the messages —
    /// safe only because reopening it refetches them.
    ///
    /// A kind that awaits no snapshot ([`Kind::awaits_snapshot`]) has nowhere
    /// to refetch from, so demoting it would throw its log away and leave it
    /// rendering empty for good. Those stay `Full`.
    fn make_partial(&mut self) {
        if let History::Full {
            kind,
            messages,
            unread_count,
            read_marker,
            ..
        } = self
            && kind.awaits_snapshot()
        {
            *self = Self::Partial {
                kind: kind.clone(),
                pending_messages: vec![],
                unread_count: *unread_count,
                max_triggers_unread: metadata::latest_triggers_unread(messages),
                read_marker: *read_marker,
                show_in_sidebar: true,
            };
        }
    }

    pub fn mark_as_read(&mut self) -> Option<ReadMarker> {
        let (unread_count, read_marker, latest) = match self {
            History::Partial {
                unread_count,
                max_triggers_unread,
                read_marker,
                ..
            } => (
                unread_count,
                read_marker,
                max_triggers_unread.map(ReadMarker::from),
            ),
            History::Full {
                messages,
                unread_count,
                read_marker,
                display_read_marker,
                ..
            } => {
                let latest = ReadMarker::latest(messages);

                if latest > *display_read_marker {
                    *display_read_marker = latest;
                }

                (unread_count, read_marker, latest)
            }
        };

        *unread_count = 0;

        if latest > *read_marker {
            *read_marker = latest;

            latest
        } else {
            None
        }
    }

    pub fn can_mark_as_read(&self) -> bool {
        match self {
            History::Partial { .. } => self.has_unread(),
            History::Full {
                messages,
                read_marker,
                ..
            } => {
                if messages.is_empty() {
                    self.unread_count() > 0
                } else {
                    self.unread_count() > 0
                        || *read_marker < ReadMarker::latest(messages)
                }
            }
        }
    }

    pub fn update_read_marker(&mut self, read_marker: ReadMarker) -> bool {
        let stored = match self {
            History::Partial {
                read_marker: stored_read_marker,
                ..
            } => stored_read_marker,
            History::Full {
                display_read_marker,
                read_marker: stored_read_marker,
                ..
            } => {
                *display_read_marker =
                    (*display_read_marker).max(Some(read_marker));
                stored_read_marker
            }
        };

        if Some(read_marker) > *stored {
            *stored = Some(read_marker);
            true
        } else {
            false
        }
    }

    pub fn read_marker(&self) -> Option<ReadMarker> {
        match self {
            History::Partial { read_marker, .. }
            | History::Full { read_marker, .. } => *read_marker,
        }
    }

    pub fn update_display_read_marker(&mut self, read_marker: ReadMarker) {
        if let History::Full {
            display_read_marker,
            ..
        } = self
        {
            *display_read_marker =
                (*display_read_marker).max(Some(read_marker));
        }
    }

    pub fn display_read_marker(&self) -> Option<ReadMarker> {
        match self {
            History::Partial { .. } => None,
            History::Full {
                display_read_marker,
                ..
            } => *display_read_marker,
        }
    }

    pub fn hide_preview(&mut self, message: message::Hash, url: url::Url) {
        if let Self::Full { messages, .. } = self
            && let Some(message) =
                messages.iter_mut().find(|m| m.hash == message)
        {
            message.hidden_urls.insert(url);
        }
    }

    pub fn show_preview(&mut self, message: message::Hash, url: &url::Url) {
        if let Self::Full { messages, .. } = self
            && let Some(message) =
                messages.iter_mut().find(|m| m.hash == message)
        {
            message.hidden_urls.remove(url);
        }
    }
}

/// Inserts `message` into `messages` sorted by `server_time`, returning
/// whether it was inserted.
///
/// For a kind that awaits a snapshot, a stored message within a
/// [`FUZZ_SECONDS`] window with the same direction, same source and equal
/// content is treated as the same message and this one is dropped — the only
/// defense when a `get_messages` snapshot races the live events that deliver
/// the same message with slightly different timestamps.
///
/// Kinds that await no snapshot ([`Kind::awaits_snapshot`]) never
/// deduplicate. Nothing races them, and a log is an append-only stream rather
/// than a delivery: real daemon output repeats itself constantly and
/// legitimately — a retry loop, a heartbeat, the same warning every tick —
/// and the rate of a repeat is frequently the whole signal (`logos-modules.md` §2b: one
/// catch-up emitted the same `newBlock` line 1158 times). Since
/// [`message::Content`]'s `PartialEq` compares rendered text alone, applying
/// the chat heuristic there drops those lines with nothing on screen to say
/// so, and the pane quietly disagrees with the file it claims to be tailing.
pub fn insert_message(
    kind: &Kind,
    messages: &mut Vec<Message>,
    message: Message,
) -> bool {
    if messages.is_empty() {
        messages.push(message);

        return true;
    }

    let dedupe = kind.awaits_snapshot();

    let fuzz = chrono::Duration::seconds(FUZZ_SECONDS);
    let start = message.server_time - fuzz;
    let end = message.server_time + fuzz;

    let start_index = messages
        .binary_search_by(|stored| stored.server_time.cmp(&start))
        .unwrap_or_else(|sorted_insert_index| sorted_insert_index);
    let end_index = messages
        .binary_search_by(|stored| stored.server_time.cmp(&end))
        .unwrap_or_else(|sorted_insert_index| sorted_insert_index);

    let mut insert_at = start_index;

    for (current_index, stored) in
        (start_index..).zip(messages[start_index..end_index].iter())
    {
        if dedupe
            && stored.direction == message.direction
            && stored.target.source() == message.target.source()
            && stored.content == message.content
        {
            return false;
        }

        if message.server_time >= stored.server_time {
            insert_at = current_index + 1;
        }
    }

    messages.insert(insert_at, message);

    true
}

#[derive(Debug)]
pub struct View<'a> {
    pub total: usize,
    pub has_more_older_messages: bool,
    pub has_more_newer_messages: bool,
    pub old_messages: Vec<&'a Message>,
    pub new_messages: Vec<&'a Message>,
    pub cleared: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::StatusKind;

    fn received(convo: &str, sender: &str, text: &str, ms: i64) -> Message {
        Message::received(
            ConvoId::from(convo),
            Some(crate::Address::from(sender)),
            text.to_string(),
            ms,
        )
    }

    /// One line of a module log, at the daemon's own wall clock.
    fn module_log(module: &str, stamp: &str, text: &str) -> Message {
        Message::module_log(crate::module::tail::Line {
            module: ModuleId::from(module),
            record: crate::module::log::parse(&format!(
                "[2026-07-31 21:32:{stamp}] [out] [{module}] {text}"
            )),
        })
    }

    fn texts(history: &History) -> Vec<String> {
        let History::Full { messages, .. } = history else {
            panic!("a log history is born Full")
        };

        messages
            .iter()
            .map(|message| message.content.text().to_string())
            .collect()
    }

    #[test]
    fn insert_message_dedupes_within_fuzz_window() {
        let kind = Kind::Conversation(ConvoId::from("c1"));
        let mut messages = vec![];

        assert!(insert_message(
            &kind,
            &mut messages,
            received("c1", "peer", "hello", 10_000)
        ));
        // Same source + content within 1s -> duplicate (snapshot racing live)
        assert!(!insert_message(
            &kind,
            &mut messages,
            received("c1", "peer", "hello", 10_400)
        ));
        // Outside the window -> genuinely repeated message
        assert!(insert_message(
            &kind,
            &mut messages,
            received("c1", "peer", "hello", 12_500)
        ));
        // Same content, different source -> kept
        assert!(insert_message(
            &kind,
            &mut messages,
            received("c1", "other", "hello", 10_400)
        ));
        // Same content + window, different direction -> kept
        assert!(insert_message(
            &kind,
            &mut messages,
            Message::sent(ConvoId::from("c1"), "hello".to_string(), 10_400)
        ));

        assert_eq!(messages.len(), 4);
        assert!(
            messages.is_sorted_by_key(|message| message.server_time),
            "insert_message must keep messages sorted"
        );
    }

    /// The dedupe heuristic is delivery reconciliation, and a log is not a
    /// delivery. Daemon output repeats itself constantly and on purpose — a
    /// retry loop, a heartbeat, the same warning every tick — and the repeat
    /// rate is frequently the only thing the pane has to show. Dropping the
    /// second copy makes the pane disagree with the file with nothing on
    /// screen to say so.
    #[test]
    fn a_log_kind_keeps_every_repeat_of_a_line() {
        // Every line inside one second, which is the whole fuzz window.
        let emitted = [
            "Waiting for peers",
            "Waiting for peers",
            "retrying dial",
            "Waiting for peers",
            "Waiting for peers",
        ];

        let mut module =
            History::new(Kind::Module(ModuleId::from("delivery_module")));

        for text in emitted {
            module.add_message(module_log("delivery_module", "59.364", text));
        }

        assert_eq!(
            texts(&module),
            emitted,
            "a repeated log line is the daemon repeating itself, not a \
             double delivery — and arrival order is the reading order",
        );

        // The app's own log pane reads the same insertion path.
        let mut logs = History::new(Kind::Logs);

        for _ in 0..20 {
            logs.add_message(Message::log(crate::log::Record {
                timestamp: chrono::DateTime::from_timestamp_millis(10_000)
                    .unwrap(),
                level: crate::log::Level::Warn,
                message: "connection refused".to_string(),
            }));
        }

        assert_eq!(texts(&logs).len(), 20);
    }

    /// The other half of the same rule: a conversation's double delivery is
    /// still collapsed, at the same level the log lines above went through.
    #[test]
    fn a_conversation_still_collapses_a_double_delivery() {
        let mut conversation =
            History::new(Kind::Conversation(ConvoId::from("c1")));

        conversation.add_message(received("c1", "peer", "hello", 10_000));
        conversation.add_message(received("c1", "peer", "hello", 10_400));

        let History::Partial {
            pending_messages, ..
        } = &conversation
        else {
            panic!("a conversation is born Partial")
        };

        assert_eq!(pending_messages.len(), 1);
    }

    #[test]
    fn add_message_tracks_unread_in_both_states() {
        let kind = Kind::Conversation(ConvoId::from("c1"));

        let mut partial = History::partial(kind.clone());
        partial.add_message(received("c1", "peer", "hi", 10_000));
        partial.add_message(Message::sent(
            ConvoId::from("c1"),
            "reply".to_string(),
            11_000,
        ));
        assert_eq!(partial.unread_count(), 1);
        assert!(partial.has_unread());

        // A Partial is not a black hole: it holds what it was given until
        // `load_full` folds the module snapshot in around it
        let History::Partial {
            pending_messages, ..
        } = &partial
        else {
            unreachable!("History::partial builds a Partial")
        };
        assert_eq!(pending_messages.len(), 2);

        let mut full = History::Full {
            kind,
            messages: vec![],
            unread_count: 0,
            read_marker: None,
            display_read_marker: None,
            cleared: false,
        };
        full.add_message(received("c1", "peer", "hi", 10_000));
        // Duplicate within the fuzz window must not bump the counter
        full.add_message(received("c1", "peer", "hi", 10_300));
        full.add_message(Message::status(
            ConvoId::from("c1"),
            StatusKind::MemberChange,
            "member joined".to_string(),
        ));
        assert_eq!(full.unread_count(), 1);

        assert!(full.mark_as_read().is_some());
        assert_eq!(full.unread_count(), 0);
        assert!(!full.has_unread());
    }

    /// The `Partial` trap: a kind whose messages are produced locally has no
    /// snapshot coming, so being born `Partial` would park every line in
    /// `pending_messages` where the view cannot reach it. Conversations do get
    /// a snapshot and must keep starting `Partial`.
    #[test]
    fn only_kinds_awaiting_a_snapshot_are_born_partial() {
        assert!(matches!(
            History::new(Kind::Conversation(ConvoId::from("c1"))),
            History::Partial { .. }
        ));
        assert!(matches!(History::new(Kind::Logs), History::Full { .. }));
        assert!(matches!(
            History::new(Kind::Module(ModuleId::from("blockchain_module"))),
            History::Full { .. }
        ));
    }

    /// Demotion throws the messages away. That is only ever safe when
    /// reopening refetches them, which a locally produced log never does.
    #[test]
    fn make_partial_spares_kinds_that_await_no_snapshot() {
        let mut module =
            History::new(Kind::Module(ModuleId::from("blockchain_module")));
        module.add_message(received("c1", "peer", "block 1", 10_000));
        module.make_partial();
        let History::Full { messages, .. } = &module else {
            panic!("a module log must not be demoted — nothing would refill it")
        };
        assert_eq!(messages.len(), 1);

        let mut logs = History::new(Kind::Logs);
        logs.make_partial();
        assert!(matches!(logs, History::Full { .. }));

        // A conversation's demotion is real and must be left alone
        let mut conversation = History::Full {
            kind: Kind::Conversation(ConvoId::from("c1")),
            messages: vec![received("c1", "peer", "hi", 10_000)],
            unread_count: 0,
            read_marker: None,
            display_read_marker: None,
            cleared: false,
        };
        conversation.make_partial();
        assert!(matches!(conversation, History::Partial { .. }));
    }

    /// A module log is a tail, not an archive: it evicts at a fifth of a
    /// conversation's cap, and in smaller bites so a syncing node does not
    /// throw away a quarter of the pane every few seconds.
    #[test]
    fn a_module_log_evicts_at_its_own_smaller_cap() {
        let kind = Kind::Module(ModuleId::from("delivery_module"));
        let mut messages: Vec<Message> = (0..MAX_MESSAGES_MODULE)
            .map(|index| received("c1", "peer", "line", 10_000 + index as i64))
            .collect();

        truncate_messages(&kind, &mut messages);
        assert_eq!(
            messages.len(),
            MAX_MESSAGES_MODULE,
            "the cap itself is not over the cap",
        );

        messages.push(received("c1", "peer", "one more", 90_000));
        truncate_messages(&kind, &mut messages);
        assert_eq!(
            messages.len(),
            MAX_MESSAGES_MODULE + 1 - TRUNC_COUNT_MODULE
        );

        // A conversation keeps the far larger cap it always had.
        let conversation = Kind::Conversation(ConvoId::from("c1"));
        let mut messages: Vec<Message> = (0..MAX_MESSAGES_MODULE + 1)
            .map(|index| received("c1", "peer", "line", 10_000 + index as i64))
            .collect();
        truncate_messages(&conversation, &mut messages);
        assert_eq!(messages.len(), MAX_MESSAGES_MODULE + 1);
    }

    #[test]
    fn module_buffers_map_to_module_histories() {
        let module_id = ModuleId::from("blockchain_module");
        let kind = Kind::from_buffer(Buffer::Module(module_id.clone()))
            .expect("a module buffer always has a history");

        assert_eq!(kind, Kind::Module(module_id.clone()));
        assert_eq!(kind.module_id(), Some(&module_id));
        assert!(kind.convo_id().is_none());
        assert_eq!(Buffer::from(kind), Buffer::Module(module_id));
    }

    #[test]
    fn mark_unread_bumps_without_messages() {
        let mut history =
            History::partial(Kind::Conversation(ConvoId::from("c1")));
        assert!(!history.has_unread());

        history.mark_unread();
        assert_eq!(history.unread_count(), 1);
        assert!(history.has_unread());
    }
}
