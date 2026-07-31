//! Domain conversations keyed by `ConvoId` — the buffer/history/config key
//! that replaces the IRC-era Server/Channel/Query taxonomy. Wire records
//! from `logos-chat` fold into these (ms timestamps become `DateTime<Utc>`,
//! member lists are loaded separately and carried across snapshots).

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::address::{self, Address};

/// Preview cap; mirrors chat_module's own 160-char truncation so a live
/// preview and a snapshot preview agree.
pub const PREVIEW_MAX_CHARS: usize = 160;

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ConvoId(Arc<str>);

impl ConvoId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn short_label(&self) -> &str {
        address::short_label(&self.0)
    }
}

impl fmt::Display for ConvoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ConvoId {
    fn from(id: &str) -> Self {
        Self(Arc::from(id))
    }
}

impl From<String> for ConvoId {
    fn from(id: String) -> Self {
        Self(Arc::from(id))
    }
}

impl From<logos_chat::ConvoId> for ConvoId {
    fn from(id: logos_chat::ConvoId) -> Self {
        Self::from(id.0)
    }
}

impl From<&logos_chat::ConvoId> for ConvoId {
    fn from(id: &logos_chat::ConvoId) -> Self {
        Self::from(id.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Direct,
    Group,
}

impl From<logos_chat::Kind> for Kind {
    fn from(kind: logos_chat::Kind) -> Self {
        match kind {
            logos_chat::Kind::Direct => Self::Direct,
            logos_chat::Kind::Group => Self::Group,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// `None` while the member has no confirmed account (empty string on
    /// the wire).
    pub address: Option<Address>,
    pub is_self: bool,
    pub pending: bool,
}

#[derive(Debug, Clone)]
pub struct Conversation {
    pub id: ConvoId,
    pub kind: Kind,
    pub nickname: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub preview: Option<String>,
    pub last_activity: DateTime<Utc>,
    pub message_count: usize,
    pub members: Vec<Member>,
}

impl Conversation {
    pub fn new(id: ConvoId, kind: Kind, last_activity: DateTime<Utc>) -> Self {
        Self {
            id,
            kind,
            nickname: None,
            name: None,
            description: None,
            preview: None,
            last_activity,
            message_count: 0,
            members: vec![],
        }
    }

    /// Local nickname wins, then the shared name, else a generated label
    /// from the conversation id (QML `fallbackDisplayName` parity).
    pub fn display_name(&self) -> String {
        if let Some(nickname) = self.nickname.as_deref()
            && !nickname.is_empty()
        {
            return nickname.to_owned();
        }

        if let Some(name) = self.name.as_deref()
            && !name.is_empty()
        {
            return name.to_owned();
        }

        match self.kind {
            Kind::Direct => format!("DM {}", self.id.short_label()),
            Kind::Group => format!("Group {}", self.id.short_label()),
        }
    }

    /// Committed roster size; a pending invite does not count towards
    /// the group until they join (QML `memberCount` parity).
    pub fn joined_member_count(&self) -> usize {
        self.members.iter().filter(|member| !member.pending).count()
    }

    /// Outstanding invitations (QML `pendingMemberCount` parity).
    pub fn pending_member_count(&self) -> usize {
        self.members.iter().filter(|member| member.pending).count()
    }

    /// The other side of a direct conversation, once members are known.
    pub fn peer_address(&self) -> Option<&Address> {
        if self.kind != Kind::Direct {
            return None;
        }

        self.members
            .iter()
            .find(|member| !member.is_self)?
            .address
            .as_ref()
    }

    /// Sets the preview from message content, truncated to
    /// [`PREVIEW_MAX_CHARS`] on a char boundary.
    pub fn set_preview_from(&mut self, content: &str) {
        let truncated = content
            .char_indices()
            .nth(PREVIEW_MAX_CHARS)
            .map_or(content, |(index, _)| &content[..index]);
        self.preview = Some(truncated.to_owned());
    }
}

impl From<logos_chat::Conversation> for Conversation {
    fn from(wire: logos_chat::Conversation) -> Self {
        let non_empty =
            |value: Option<String>| value.filter(|value| !value.is_empty());

        Self {
            id: ConvoId::from(wire.convo_id),
            kind: Kind::from(wire.kind),
            nickname: non_empty(wire.nickname),
            name: non_empty(wire.name),
            description: non_empty(wire.description),
            preview: non_empty(wire.preview),
            last_activity: DateTime::from_timestamp_millis(
                wire.last_activity_ms,
            )
            .unwrap_or_default(),
            message_count: usize::try_from(wire.message_count)
                .unwrap_or_default(),
            members: vec![],
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Map(IndexMap<ConvoId, Conversation>);

impl Map {
    pub fn get(&self, id: &ConvoId) -> Option<&Conversation> {
        self.0.get(id)
    }

    pub fn get_mut(&mut self, id: &ConvoId) -> Option<&mut Conversation> {
        self.0.get_mut(id)
    }

    pub fn contains(&self, id: &ConvoId) -> bool {
        self.0.contains_key(id)
    }

    pub fn insert(
        &mut self,
        conversation: Conversation,
    ) -> Option<Conversation> {
        self.0.insert(conversation.id.clone(), conversation)
    }

    pub fn remove(&mut self, id: &ConvoId) -> Option<Conversation> {
        self.0.shift_remove(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Conversation> {
        self.0.values()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Conversation> {
        self.0.values_mut()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Replaces all rows from a wire snapshot, carrying over locally-known
    /// member lists where ids match (members are loaded separately and
    /// snapshots don't include them — QML parity with `unreadCounts`
    /// carried across `rehydrateConversations`).
    pub fn replace_all(
        &mut self,
        conversations: Vec<logos_chat::Conversation>,
    ) {
        let mut next = IndexMap::with_capacity(conversations.len());

        for wire in conversations {
            let mut conversation = Conversation::from(wire);

            if let Some(existing) = self.0.swap_remove(&conversation.id) {
                conversation.members = existing.members;
            }

            next.insert(conversation.id.clone(), conversation);
        }

        self.0 = next;
    }

    /// The sidebar order: most recent activity first, display name as the
    /// tie-breaker.
    pub fn sorted(&self) -> Vec<&Conversation> {
        let mut conversations = self.0.values().collect::<Vec<_>>();

        conversations.sort_by(|a, b| {
            b.last_activity
                .cmp(&a.last_activity)
                .then_with(|| a.display_name().cmp(&b.display_name()))
        });

        conversations
    }

    /// [`Self::sorted`] partitioned into (direct, group).
    pub fn sectioned(&self) -> (Vec<&Conversation>, Vec<&Conversation>) {
        self.sorted()
            .into_iter()
            .partition(|conversation| conversation.kind == Kind::Direct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(id: &str, kind: Kind, ms: i64) -> Conversation {
        Conversation::new(
            ConvoId::from(id),
            kind,
            DateTime::from_timestamp_millis(ms).unwrap(),
        )
    }

    fn wire(id: &str, ms: i64) -> logos_chat::Conversation {
        logos_chat::Conversation {
            convo_id: logos_chat::ConvoId(id.to_owned()),
            nickname: None,
            message_count: 0,
            last_activity_ms: ms,
            kind: logos_chat::Kind::Direct,
            name: None,
            description: None,
            preview: None,
        }
    }

    #[test]
    fn display_name_precedence() {
        let mut convo = conversation("deadbeef1234", Kind::Group, 0);
        assert_eq!(convo.display_name(), "Group deadbeef");

        convo.name = Some("Team".to_owned());
        assert_eq!(convo.display_name(), "Team");

        convo.nickname = Some("pals".to_owned());
        assert_eq!(convo.display_name(), "pals");

        let direct = conversation("cafef00d5678", Kind::Direct, 0);
        assert_eq!(direct.display_name(), "DM cafef00d");
    }

    #[test]
    fn set_preview_truncates_on_char_boundary() {
        let mut convo = conversation("c1", Kind::Direct, 0);

        convo.set_preview_from("short");
        assert_eq!(convo.preview.as_deref(), Some("short"));

        let long = "é".repeat(PREVIEW_MAX_CHARS + 40);
        convo.set_preview_from(&long);
        assert_eq!(
            convo.preview.as_deref(),
            Some("é".repeat(PREVIEW_MAX_CHARS).as_str())
        );
    }

    #[test]
    fn joined_member_count_excludes_pending_invites() {
        let mut convo = conversation("g1", Kind::Group, 0);
        assert_eq!(convo.joined_member_count(), 0);

        convo.members = vec![
            Member {
                address: Some(Address::from("me")),
                is_self: true,
                pending: false,
            },
            Member {
                address: None,
                is_self: false,
                pending: true,
            },
            Member {
                address: Some(Address::from("peer")),
                is_self: false,
                pending: false,
            },
        ];
        assert_eq!(convo.joined_member_count(), 2);
        assert_eq!(convo.pending_member_count(), 1);
    }

    #[test]
    fn peer_address_is_first_non_self_member_of_a_direct() {
        let mut convo = conversation("c1", Kind::Direct, 0);
        assert_eq!(convo.peer_address(), None);

        convo.members = vec![
            Member {
                address: Some(Address::from("me")),
                is_self: true,
                pending: false,
            },
            Member {
                address: Some(Address::from("peer")),
                is_self: false,
                pending: false,
            },
        ];
        assert_eq!(convo.peer_address(), Some(&Address::from("peer")));

        convo.kind = Kind::Group;
        assert_eq!(convo.peer_address(), None);
    }

    #[test]
    fn sorted_orders_by_recency_then_display_name() {
        let mut map = Map::default();
        map.insert(conversation("older", Kind::Direct, 1_000));
        map.insert(conversation("bbbbbbbb", Kind::Direct, 2_000));
        map.insert(conversation("aaaaaaaa", Kind::Direct, 2_000));

        let ids = map
            .sorted()
            .into_iter()
            .map(|convo| convo.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["aaaaaaaa", "bbbbbbbb", "older"]);
    }

    #[test]
    fn sectioned_partitions_directs_from_groups() {
        let mut map = Map::default();
        map.insert(conversation("d1", Kind::Direct, 2_000));
        map.insert(conversation("g1", Kind::Group, 3_000));
        map.insert(conversation("d2", Kind::Direct, 1_000));

        let (direct, group) = map.sectioned();
        let ids = |convos: Vec<&Conversation>| {
            convos
                .into_iter()
                .map(|convo| convo.id.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(direct), ["d1", "d2"]);
        assert_eq!(ids(group), ["g1"]);
    }

    #[test]
    fn replace_all_carries_members_for_matching_ids() {
        let mut map = Map::default();
        let mut existing = conversation("kept", Kind::Group, 1_000);
        existing.members = vec![Member {
            address: Some(Address::from("peer")),
            is_self: false,
            pending: true,
        }];
        map.insert(existing);
        map.insert(conversation("dropped", Kind::Direct, 1_000));

        map.replace_all(vec![wire("kept", 5_000), wire("fresh", 4_000)]);

        assert_eq!(map.len(), 2);
        assert!(!map.contains(&ConvoId::from("dropped")));

        let kept = map.get(&ConvoId::from("kept")).unwrap();
        assert_eq!(kept.members.len(), 1);
        assert_eq!(kept.members[0].address, Some(Address::from("peer")));
        assert_eq!(
            kept.last_activity,
            DateTime::from_timestamp_millis(5_000).unwrap()
        );
        assert!(map.get(&ConvoId::from("fresh")).unwrap().members.is_empty());
    }
}
