use std::collections::{HashMap, HashSet};

use crate::conversation::ConvoId;
use crate::history::{self, History, ReadMarker};
use crate::message::Limit;
use crate::module::ModuleId;
use crate::{Message, input};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Resource {
    pub kind: history::Kind,
}

impl Resource {
    pub fn logs() -> Self {
        Self {
            kind: history::Kind::Logs,
        }
    }

    pub fn module(module_id: ModuleId) -> Self {
        Self {
            kind: history::Kind::Module(module_id),
        }
    }
}

#[derive(Debug, Default)]
pub struct Manager {
    resources: HashSet<Resource>,
    data: Data,
}

impl Manager {
    /// Syncs the set of tracked histories with the currently open buffers.
    /// Newly tracked kinds get an entry so unread state accrues — `Partial`
    /// for conversations, `Full` for kinds that await no snapshot (see
    /// [`History::new`]). Untracked conversations collapse back to `Partial`
    /// (the module is the message store — nothing to flush); the rest
    /// keep their `Full` state, since nothing would ever refill them.
    pub fn track(&mut self, new_resources: HashSet<Resource>) {
        let added = new_resources
            .difference(&self.resources)
            .cloned()
            .collect::<Vec<_>>();
        let removed = self
            .resources
            .difference(&new_resources)
            .cloned()
            .collect::<Vec<_>>();

        for resource in added {
            self.data
                .map
                .entry(resource.kind.clone())
                .or_insert_with(|| History::new(resource.kind));
        }

        for resource in removed {
            self.data.make_partial(&resource.kind);
        }

        self.resources = new_resources;
    }

    pub fn is_tracked(&self, kind: &history::Kind) -> bool {
        self.resources.contains(&Resource { kind: kind.clone() })
    }

    /// Folds a backend `get_messages` snapshot into a `Full` history.
    /// Messages recorded while the snapshot was in flight — a `Partial`'s
    /// `pending_messages`, or live events landing on an already `Full`
    /// history — are merged through `insert_message`'s dedupe window.
    pub fn load_full(&mut self, kind: history::Kind, messages: Vec<Message>) {
        self.data.load_full(kind, messages);
    }

    pub fn open(&mut self, kind: history::Kind) {
        let history = self
            .data
            .map
            .entry(kind.clone())
            .or_insert_with(|| History::new(kind));

        if let History::Partial {
            show_in_sidebar, ..
        } = history
        {
            *show_in_sidebar = true;
        }
    }

    pub fn close(&mut self, kind: &history::Kind) {
        self.data.map.remove(kind);
    }

    pub fn make_partial(&mut self, kind: &history::Kind) {
        self.data.make_partial(kind);
    }

    pub fn clear_messages(&mut self, kind: &history::Kind) {
        if let Some(History::Full {
            messages,
            unread_count,
            cleared,
            ..
        }) = self.data.map.get_mut(kind)
        {
            messages.clear();
            *unread_count = 0;
            *cleared = true;

            log::debug!("cleared messages for {kind}");
        }
    }

    pub fn record_message(&mut self, kind: history::Kind, message: Message) {
        self.data.add_message(kind, message);
    }

    pub fn record_log(&mut self, record: crate::log::Record) {
        self.data
            .add_message(history::Kind::Logs, Message::log(record));
    }

    /// Files one tailed log line under the module it was attributed to.
    ///
    /// Takes the id from the line rather than from the caller: attribution is
    /// resolved once, in the parser, and a caller that could disagree with it
    /// would be a second place for a line to end up on the wrong pane.
    pub fn record_module_log(&mut self, line: crate::module::tail::Line) {
        self.data.add_message(
            history::Kind::Module(line.module.clone()),
            Message::module_log(line),
        );
    }

    pub fn record_input_history(&mut self, convo_id: &ConvoId, text: String) {
        self.data.input.record(convo_id, text);
    }

    pub fn record_draft(&mut self, raw_input: input::RawInput) {
        self.data.input.store_draft(raw_input);
    }

    pub fn input<'a>(&'a self, convo_id: &ConvoId) -> input::Cache<'a> {
        self.data.input.get(convo_id)
    }

    pub fn get_messages(
        &self,
        kind: &history::Kind,
        limit: Option<Limit>,
    ) -> Option<history::View<'_>> {
        self.data.history_view(kind, limit)
    }

    /// Whether a pane on `kind` would render nothing.
    ///
    /// Answers the same question as an empty [`Self::get_messages`] without
    /// building the view: that walks and clones a reference per message, and
    /// an "is there anything here" check runs on every frame. `Partial`
    /// counts as empty because its pending messages are deliberately
    /// invisible until a snapshot merges them.
    pub fn is_empty(&self, kind: &history::Kind) -> bool {
        match self.data.map.get(kind) {
            Some(History::Full { messages, .. }) => messages.is_empty(),
            Some(History::Partial { .. }) | None => true,
        }
    }

    pub fn mark_as_read(&mut self, kind: &history::Kind) -> Option<ReadMarker> {
        self.data.map.get_mut(kind).and_then(History::mark_as_read)
    }

    pub fn can_mark_as_read(&self, kind: &history::Kind) -> bool {
        self.data
            .map
            .get(kind)
            .is_some_and(History::can_mark_as_read)
    }

    /// Explicit unread bump for events that carry no message, e.g. an
    /// incoming group invite (`conversation_created` we didn't initiate).
    pub fn mark_unread(&mut self, kind: &history::Kind) {
        self.data
            .map
            .entry(kind.clone())
            .or_insert_with(|| History::new(kind.clone()))
            .mark_unread();
    }

    pub fn unread_count(&self, kind: &history::Kind) -> usize {
        self.data
            .map
            .get(kind)
            .map(History::unread_count)
            .unwrap_or_default()
    }

    pub fn has_unread(&self, kind: &history::Kind) -> bool {
        self.data.map.get(kind).is_some_and(History::has_unread)
    }

    pub fn update_read_marker(
        &mut self,
        kind: &history::Kind,
        read_marker: ReadMarker,
    ) -> bool {
        self.data
            .map
            .get_mut(kind)
            .is_some_and(|history| history.update_read_marker(read_marker))
    }

    pub fn update_display_read_marker(
        &mut self,
        kind: &history::Kind,
        read_marker: ReadMarker,
    ) {
        if let Some(history) = self.data.map.get_mut(kind) {
            history.update_display_read_marker(read_marker);
        }
    }

    pub fn read_marker(
        &self,
        kind: &history::Kind,
    ) -> Option<history::ReadMarker> {
        self.data
            .map
            .get(kind)
            .map(History::read_marker)
            .unwrap_or_default()
    }

    pub fn kinds(&self) -> Vec<history::Kind> {
        self.data.map.keys().cloned().collect()
    }

    pub fn is_preview_hidden(
        &self,
        kind: &history::Kind,
        hash: crate::message::Hash,
        server_time: chrono::DateTime<chrono::Utc>,
        url: &url::Url,
    ) -> bool {
        self.data.is_preview_hidden(kind, hash, server_time, url)
    }

    pub fn hide_preview(
        &mut self,
        kind: impl Into<history::Kind>,
        message: crate::message::Hash,
        url: url::Url,
    ) {
        if let Some(history) = self.data.map.get_mut(&kind.into()) {
            history.hide_preview(message, url);
        }
    }

    pub fn show_preview(
        &mut self,
        kind: impl Into<history::Kind>,
        message: crate::message::Hash,
        url: &url::Url,
    ) {
        if let Some(history) = self.data.map.get_mut(&kind.into()) {
            history.show_preview(message, url);
        }
    }
}

fn with_limit<'a>(
    limit: Option<Limit>,
    messages: impl Iterator<Item = &'a Message>,
) -> Vec<&'a Message> {
    match limit {
        Some(Limit::Top(n)) => messages.take(n).collect(),
        Some(Limit::Bottom(n)) => {
            let collected = messages.collect::<Vec<_>>();
            let length = collected.len();
            collected[length.saturating_sub(n)..length].to_vec()
        }
        Some(Limit::Since(timestamp)) => messages
            .skip_while(|message| message.server_time < timestamp)
            .collect(),
        Some(Limit::Around(n, hash)) => {
            let collected = messages.collect::<Vec<_>>();
            let length = collected.len();
            let center = collected
                .iter()
                .position(|m| m.hash == hash)
                .unwrap_or(length.saturating_sub(1));
            let start =
                center.saturating_sub(n / 2).min(length.saturating_sub(n));
            let end = (start + n).min(length);
            collected[start..end].to_vec()
        }
        None => messages.collect(),
    }
}

#[derive(Debug, Default)]
struct Data {
    map: HashMap<history::Kind, History>,
    input: input::Storage,
}

impl Data {
    fn load_full(&mut self, kind: history::Kind, snapshot: Vec<Message>) {
        let len = snapshot.len();

        match self.map.remove(&kind) {
            Some(History::Full {
                mut messages,
                unread_count,
                read_marker,
                display_read_marker,
                cleared,
                ..
            }) => {
                for message in snapshot {
                    history::insert_message(&kind, &mut messages, message);
                }

                history::truncate_messages(&kind, &mut messages);

                self.map.insert(
                    kind.clone(),
                    History::Full {
                        kind: kind.clone(),
                        messages,
                        unread_count,
                        read_marker,
                        display_read_marker,
                        cleared,
                    },
                );
            }
            previous => {
                let (unread_count, read_marker, mut messages) = match previous {
                    Some(History::Partial {
                        pending_messages,
                        unread_count,
                        read_marker,
                        ..
                    }) => (unread_count, read_marker, pending_messages),
                    _ => (0, None, Vec::new()),
                };

                messages.reserve(len);

                for message in snapshot {
                    history::insert_message(&kind, &mut messages, message);
                }

                history::truncate_messages(&kind, &mut messages);

                self.map.insert(
                    kind.clone(),
                    History::Full {
                        kind: kind.clone(),
                        messages,
                        unread_count,
                        read_marker,
                        display_read_marker: read_marker,
                        cleared: false,
                    },
                );
            }
        }

        log::debug!("loaded history for {kind}: {len} messages");
    }

    fn make_partial(&mut self, kind: &history::Kind) {
        if let Some(history) = self.map.get_mut(kind) {
            history.make_partial();
        }
    }

    fn add_message(&mut self, kind: history::Kind, message: Message) {
        self.map
            .entry(kind.clone())
            .or_insert_with(|| History::new(kind))
            .add_message(message);
    }

    fn history_view(
        &self,
        kind: &history::Kind,
        limit: Option<Limit>,
    ) -> Option<history::View<'_>> {
        let History::Full {
            messages,
            display_read_marker,
            cleared,
            ..
        } = self.map.get(kind)?
        else {
            return None;
        };

        let total = messages.len();

        let first_without_limit = messages.first();
        let last_without_limit = messages.last();

        let limited = with_limit(limit, messages.iter());

        let first_with_limit = limited.first();
        let last_with_limit = limited.last();

        let split_at = display_read_marker.map_or(0, |display_read_marker| {
            limited
                .iter()
                .rev()
                .position(|message| {
                    message.server_time <= display_read_marker.date_time()
                })
                .map_or_else(
                    || 0, // Backlog is before this limit view of messages
                    |position| limited.len() - position,
                )
        });

        let (old, new) = limited.split_at(split_at);

        let has_more_older_messages = first_without_limit
            .zip(first_with_limit)
            .is_some_and(|(without_limit, with_limit)| {
                without_limit.hash != with_limit.hash
            });
        let has_more_newer_messages = last_without_limit
            .zip(last_with_limit)
            .is_some_and(|(without_limit, with_limit)| {
                without_limit.hash != with_limit.hash
            });

        Some(history::View {
            total,
            has_more_older_messages,
            has_more_newer_messages,
            old_messages: old.to_vec(),
            new_messages: new.to_vec(),
            cleared: *cleared,
        })
    }

    fn is_preview_hidden(
        &self,
        kind: &history::Kind,
        hash: crate::message::Hash,
        server_time: chrono::DateTime<chrono::Utc>,
        url: &url::Url,
    ) -> bool {
        let Some(History::Full { messages, .. }) = self.map.get(kind) else {
            return false;
        };
        // messages is sorted by server_time, so binary search to the right
        // timestamp then scan the same-second slice by hash
        let start = messages.partition_point(|m| m.server_time < server_time);
        messages[start..]
            .iter()
            .take_while(|m| m.server_time == server_time)
            .find(|m| m.hash == hash)
            .is_some_and(|m| m.hidden_urls.contains(url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Address;

    fn kind(id: &str) -> history::Kind {
        history::Kind::Conversation(ConvoId::from(id))
    }

    fn received(convo: &str, text: &str, ms: i64) -> Message {
        Message::received(
            ConvoId::from(convo),
            Some(Address::from("peer")),
            text.to_string(),
            ms,
        )
    }

    #[test]
    fn load_full_merges_live_messages_racing_the_snapshot() {
        let mut manager = Manager::default();
        let kind = kind("c1");

        // Live events arrive before the snapshot lands
        manager.record_message(kind.clone(), received("c1", "one", 10_000));
        manager.record_message(kind.clone(), received("c1", "two", 20_000));
        assert_eq!(manager.unread_count(&kind), 2);
        assert!(manager.get_messages(&kind, None).is_none());

        // Snapshot contains the same messages (fuzzed timestamps) plus older
        manager.load_full(
            kind.clone(),
            vec![
                received("c1", "zero", 5_000),
                received("c1", "one", 10_200),
                received("c1", "two", 20_200),
            ],
        );

        let view = manager.get_messages(&kind, None).unwrap();
        assert_eq!(view.total, 3);
        assert_eq!(manager.unread_count(&kind), 2);

        // Later snapshots into an existing Full dedupe the same way
        manager.load_full(
            kind.clone(),
            vec![
                received("c1", "zero", 5_000),
                received("c1", "one", 10_200),
                received("c1", "two", 20_200),
                received("c1", "three", 30_000),
            ],
        );

        let view = manager.get_messages(&kind, None).unwrap();
        assert_eq!(view.total, 4);
    }

    #[test]
    fn load_full_keeps_locally_generated_status_messages() {
        let mut manager = Manager::default();
        let kind = kind("c1");

        // `ConversationCreated` opens the history and writes its status line
        // before the module snapshot is even asked for
        manager.open(kind.clone());
        manager.record_message(
            kind.clone(),
            Message::status(
                ConvoId::from("c1"),
                crate::message::StatusKind::NewConversation,
                "conversation created".to_string(),
            ),
        );

        // The snapshot knows nothing of a locally generated status line
        manager.load_full(kind.clone(), vec![]);

        let view = manager.get_messages(&kind, None).unwrap();
        assert_eq!(view.total, 1);
        assert!(matches!(
            view.old_messages
                .iter()
                .chain(&view.new_messages)
                .next()
                .unwrap()
                .target
                .source(),
            crate::message::Source::Status(_)
        ));
    }

    #[test]
    fn unread_flow_increment_and_clear() {
        let mut manager = Manager::default();
        let kind = kind("c1");

        manager.load_full(kind.clone(), vec![]);
        manager.record_message(kind.clone(), received("c1", "hi", 10_000));
        assert_eq!(manager.unread_count(&kind), 1);
        assert!(manager.has_unread(&kind));
        assert!(manager.can_mark_as_read(&kind));

        assert!(manager.mark_as_read(&kind).is_some());
        assert_eq!(manager.unread_count(&kind), 0);
        assert!(!manager.has_unread(&kind));

        manager.mark_unread(&kind);
        assert_eq!(manager.unread_count(&kind), 1);

        // Group invite for a conversation we've never opened
        let invited = kind_missing();
        manager.mark_unread(&invited);
        assert_eq!(manager.unread_count(&invited), 1);
        assert!(manager.has_unread(&invited));
    }

    fn kind_missing() -> history::Kind {
        history::Kind::Conversation(ConvoId::from("fresh"))
    }

    /// The `Partial` trap, at the seam that actually renders: `get_messages`
    /// only answers for a `Full` history. Nothing will ever call `load_full`
    /// for a log, so a log that starts `Partial` renders empty for the whole
    /// session no matter how many lines it is handed.
    #[test]
    fn log_histories_render_without_ever_being_loaded() {
        let mut manager = Manager::default();
        let module = history::Kind::Module(ModuleId::from("blockchain_module"));

        manager.track(HashSet::from([
            Resource::module(ModuleId::from("blockchain_module")),
            Resource::logs(),
        ]));

        manager
            .record_message(module.clone(), received("c1", "block 1", 1_000));
        manager.record_log(crate::log::Record {
            timestamp: chrono::Utc::now(),
            level: crate::log::Level::Info,
            message: "started".to_string(),
        });

        assert_eq!(manager.get_messages(&module, None).unwrap().total, 1);
        assert_eq!(
            manager
                .get_messages(&history::Kind::Logs, None)
                .unwrap()
                .total,
            1
        );

        // Closing the pane must not take the lines with it: there is no
        // snapshot to refetch them from when it reopens
        manager.track(HashSet::new());
        assert_eq!(manager.get_messages(&module, None).unwrap().total, 1);
    }

    /// The fix above must not reach conversations, whose `Partial` state is a
    /// real promise that a `get_messages` snapshot is on its way.
    #[test]
    fn conversations_still_start_partial_and_promote_on_load() {
        let mut manager = Manager::default();
        let kind = kind("c1");

        manager.track(HashSet::from([Resource { kind: kind.clone() }]));
        manager.record_message(kind.clone(), received("c1", "hi", 10_000));
        assert!(
            manager.get_messages(&kind, None).is_none(),
            "a tracked conversation is Partial until its snapshot lands"
        );

        manager.load_full(kind.clone(), vec![received("c1", "hi", 10_100)]);
        let view = manager.get_messages(&kind, None).unwrap();
        assert_eq!(view.total, 1, "the pending message survives promotion");

        // Untracking a conversation still collapses it — reopening refetches
        manager.track(HashSet::new());
        assert!(manager.get_messages(&kind, None).is_none());
    }

    #[test]
    fn get_messages_windows_and_backlog_split() {
        let mut manager = Manager::default();
        let kind = kind("c1");

        manager.load_full(
            kind.clone(),
            (0..10)
                .map(|i| received("c1", &format!("m{i}"), i * 10_000))
                .collect(),
        );

        let marker = ReadMarker::from(
            chrono::DateTime::from_timestamp_millis(40_000).unwrap(),
        );
        manager.update_read_marker(&kind, marker);

        let view = manager.get_messages(&kind, Some(Limit::Bottom(6))).unwrap();
        assert_eq!(view.total, 10);
        assert!(view.has_more_older_messages);
        assert!(!view.has_more_newer_messages);
        assert_eq!(view.old_messages.len(), 1);
        assert_eq!(view.new_messages.len(), 5);
    }
}
