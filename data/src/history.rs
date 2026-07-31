use core::fmt;

use chrono::{DateTime, Utc};

pub use self::manager::{Manager, Resource};
pub use self::metadata::ReadMarker;
use crate::conversation::ConvoId;
use crate::{Buffer, Message, buffer, message};

pub mod manager;
pub mod metadata;

/// Persistence seam: module-side persistence lands upstream; until it does,
/// histories live in memory only. When it lands, key files on
/// `seahash("convo:{id}")` — never revive the `fmt::Binary`-of-Server scheme.
pub mod persistence {}

pub(crate) const MAX_MESSAGES: usize = 10_000;

const TRUNC_COUNT: usize = 500;

/// Messages that arrive within this window of an existing message are
/// candidates for deduplication (a `get_messages` snapshot racing live
/// events delivers the same message with slightly different timestamps).
const FUZZ_SECONDS: i64 = 1;

pub(crate) fn truncate_messages(messages: &mut Vec<Message>) {
    if messages.len() > MAX_MESSAGES {
        messages.drain(0..TRUNC_COUNT);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    Conversation(ConvoId),
    Logs,
}

impl Kind {
    pub fn from_buffer(buffer: Buffer) -> Option<Self> {
        match buffer {
            Buffer::Conversation(convo_id) => {
                Some(Kind::Conversation(convo_id))
            }
            Buffer::Internal(buffer::Internal::Logs) => Some(Kind::Logs),
            Buffer::Internal(buffer::Internal::ConfigEditor) => None,
        }
    }

    pub fn convo_id(&self) -> Option<&ConvoId> {
        match self {
            Kind::Conversation(convo_id) => Some(convo_id),
            Kind::Logs => None,
        }
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
            Kind::Logs => write!(f, "logs"),
        }
    }
}

impl From<Kind> for Buffer {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Conversation(convo_id) => Buffer::Conversation(convo_id),
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

                insert_message(pending_messages, message);

                truncate_messages(pending_messages);
            }
            History::Full {
                messages,
                unread_count,
                ..
            } => {
                if insert_message(messages, message) && triggers_unread {
                    *unread_count += 1;
                }

                truncate_messages(messages);
            }
        }
    }

    fn make_partial(&mut self) {
        if let History::Full {
            kind,
            messages,
            unread_count,
            read_marker,
            ..
        } = self
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

/// Inserts `message` into `messages` sorted by `server_time`, unless a
/// message within a [`FUZZ_SECONDS`] window with the same direction, same
/// source, and equal content already exists — the only defense when a
/// `get_messages` snapshot races live events. Returns whether the message
/// was inserted.
pub fn insert_message(messages: &mut Vec<Message>, message: Message) -> bool {
    if messages.is_empty() {
        messages.push(message);

        return true;
    }

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
        if stored.direction == message.direction
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

    #[test]
    fn insert_message_dedupes_within_fuzz_window() {
        let mut messages = vec![];

        assert!(insert_message(
            &mut messages,
            received("c1", "peer", "hello", 10_000)
        ));
        // Same source + content within 1s -> duplicate (snapshot racing live)
        assert!(!insert_message(
            &mut messages,
            received("c1", "peer", "hello", 10_400)
        ));
        // Outside the window -> genuinely repeated message
        assert!(insert_message(
            &mut messages,
            received("c1", "peer", "hello", 12_500)
        ));
        // Same content, different source -> kept
        assert!(insert_message(
            &mut messages,
            received("c1", "other", "hello", 10_400)
        ));
        // Same content + window, different direction -> kept
        assert!(insert_message(
            &mut messages,
            Message::sent(ConvoId::from("c1"), "hello".to_string(), 10_400)
        ));

        assert_eq!(messages.len(), 4);
        assert!(
            messages.is_sorted_by_key(|message| message.server_time),
            "insert_message must keep messages sorted"
        );
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
