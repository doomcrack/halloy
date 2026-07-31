//! Pure fold of backend [`logos_chat::Update`]s into domain state — the
//! replacement for the IRC-era `client::Map` message match. History/message
//! integration is wired separately by the UI; only delivery status,
//! identity, and the conversation map live here.

use chrono::{DateTime, Utc};
use logos_chat::{ChatEvent, Phase, Update};

use crate::address::Address;
use crate::conversation::{self, Conversation, ConvoId, Kind, Member};
use crate::delivery::{Delivery, DeliveryState, Identity};

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub delivery: Delivery,
    pub identity: Option<Identity>,
    pub conversations: conversation::Map,
}

impl Session {
    pub fn apply(&mut self, update: &Update) {
        match update {
            Update::Phase(phase) => self.apply_phase(phase),
            Update::Ready {
                my_address,
                installation_name,
            } => {
                let address = Address::from(my_address.as_str());

                // `LoadMembers` is deliberately ungated, so a member list
                // can land before the first `Ready`: re-derive `is_self`
                // against the fresh identity.
                for conversation in self.conversations.iter_mut() {
                    for member in &mut conversation.members {
                        member.is_self =
                            member.address.as_ref() == Some(&address);
                    }
                }

                let installation_name =
                    installation_name.clone().or_else(|| {
                        self.identity
                            .take()
                            .and_then(|identity| identity.installation_name)
                    });

                self.identity = Some(Identity {
                    address,
                    installation_name,
                });
            }
            Update::ConversationsSnapshot(conversations) => {
                self.conversations.replace_all(conversations.clone());
            }
            Update::MembersLoaded { convo_id, members } => {
                let my_address = self
                    .identity
                    .as_ref()
                    .map(|identity| identity.address.clone());

                if let Some(conversation) =
                    self.conversations.get_mut(&ConvoId::from(convo_id))
                {
                    conversation.members = members
                        .iter()
                        .map(|member| {
                            let address =
                                (!member.address.is_empty()).then(|| {
                                    Address::from(member.address.as_str())
                                });
                            let is_self =
                                address.is_some() && address == my_address;

                            Member {
                                address,
                                is_self,
                                pending: member.pending,
                            }
                        })
                        .collect();
                }
            }
            Update::Event(event) => self.apply_event(event),
            // Message history is wired by the UI; the remaining variants
            // carry no domain state.
            Update::Controller(_)
            | Update::MessagesLoaded { .. }
            | Update::ActionFailed(_)
            | Update::Fatal(_)
            | Update::Stopped => {}
        }
    }

    fn apply_phase(&mut self, phase: &Phase) {
        // The detail spells the phase out so the status bar can say more
        // than "not online" (e.g. "Restarting" vs first-boot startup).
        let (status, detail) = match phase {
            Phase::Online => (DeliveryState::Online, String::new()),
            Phase::DeliveryError { detail } => {
                (DeliveryState::Error, detail.clone())
            }
            Phase::Failed => {
                (DeliveryState::Error, "backend session failed".to_owned())
            }
            Phase::DeliveryStopped | Phase::ShuttingDown => {
                (DeliveryState::Stopped, String::new())
            }
            Phase::StartingDaemon => {
                (DeliveryState::Initialising, "Starting daemon".to_owned())
            }
            Phase::Connecting => {
                (DeliveryState::Initialising, "Connecting".to_owned())
            }
            Phase::LoadingModule => (
                DeliveryState::Initialising,
                "Loading chat module".to_owned(),
            ),
            Phase::InitialisingChat => {
                (DeliveryState::Initialising, "Initialising chat".to_owned())
            }
            Phase::Restarting { attempt } => (
                DeliveryState::Initialising,
                format!("Restarting (attempt {attempt})"),
            ),
        };

        self.delivery = Delivery { status, detail };
    }

    fn apply_event(&mut self, event: &ChatEvent) {
        match event {
            ChatEvent::MessageReceived {
                convo_id,
                content,
                timestamp_ms,
                ..
            }
            | ChatEvent::MessageSent {
                convo_id,
                content,
                timestamp_ms,
            } => {
                self.record_message(
                    ConvoId::from(convo_id),
                    content,
                    *timestamp_ms,
                );
            }
            ChatEvent::ConversationCreated {
                convo_id,
                kind,
                name,
                desc,
                ..
            } => {
                let id = ConvoId::from(convo_id);
                let name = (!name.is_empty()).then(|| name.clone());
                let description = (!desc.is_empty()).then(|| desc.clone());

                if let Some(conversation) = self.conversations.get_mut(&id) {
                    conversation.name = name;
                    conversation.description = description;
                    conversation.last_activity = Utc::now();
                } else {
                    let mut conversation =
                        Conversation::new(id, Kind::from(*kind), Utc::now());
                    conversation.name = name;
                    conversation.description = description;
                    self.conversations.insert(conversation);
                }
            }
            ChatEvent::ConversationDeleted { convo_id } => {
                self.conversations.remove(&ConvoId::from(convo_id));
            }
            // These carry only the id; the session machine refetches
            // (`conversation_updated` → resync, `members_changed` → member
            // reload), and the result arrives as
            // `ConversationsSnapshot`/`MembersLoaded`.
            ChatEvent::ConversationUpdated { .. }
            | ChatEvent::MembersChanged { .. } => {}
            ChatEvent::DeliveryStateChanged { state, detail } => {
                // Unrecognized wire states keep the previous status
                // (QML parity).
                if *state != DeliveryState::Unknown {
                    self.delivery = Delivery {
                        status: *state,
                        detail: detail.clone(),
                    };
                }
            }
        }
    }

    fn record_message(&mut self, id: ConvoId, content: &str, ms: i64) {
        let last_activity =
            DateTime::from_timestamp_millis(ms).unwrap_or_else(Utc::now);

        if let Some(conversation) = self.conversations.get_mut(&id) {
            conversation.set_preview_from(content);
            conversation.last_activity = last_activity;
            conversation.message_count += 1;
        } else {
            // Defensive: conversation_created normally lands first with
            // the kind; a resync backfills the real record (QML parity).
            let mut conversation =
                Conversation::new(id, Kind::Direct, last_activity);
            conversation.set_preview_from(content);
            conversation.message_count = 1;
            self.conversations.insert(conversation);
        }
    }
}

#[cfg(test)]
mod tests {
    use logos_chat::GroupMember;

    use super::*;

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
    fn ready_sets_identity() {
        let mut session = Session::default();
        assert_eq!(session.identity, None);

        session.apply(&Update::Ready {
            my_address: "deadbeef1234".to_owned(),
            installation_name: None,
        });

        let identity = session.identity.as_ref().unwrap();
        assert_eq!(identity.address, Address::from("deadbeef1234"));
        assert_eq!(identity.installation_name, None);

        // A later resync carries the name; a nameless one keeps it.
        session.apply(&Update::Ready {
            my_address: "deadbeef1234".to_owned(),
            installation_name: Some("laptop".to_owned()),
        });
        let identity = session.identity.as_ref().unwrap();
        assert_eq!(identity.installation_name.as_deref(), Some("laptop"));

        session.apply(&Update::Ready {
            my_address: "deadbeef1234".to_owned(),
            installation_name: None,
        });
        let identity = session.identity.as_ref().unwrap();
        assert_eq!(identity.installation_name.as_deref(), Some("laptop"));
    }

    #[test]
    fn snapshot_then_message_received_bumps_preview_and_activity() {
        let mut session = Session::default();
        session.apply(&Update::ConversationsSnapshot(vec![
            wire("c1", 1_000),
            wire("c2", 2_000),
        ]));
        assert_eq!(session.conversations.len(), 2);

        session.apply(&Update::Event(ChatEvent::MessageReceived {
            convo_id: logos_chat::ConvoId("c1".to_owned()),
            content: "hello there".to_owned(),
            timestamp_ms: 9_000,
            sender: "peer".to_owned(),
        }));

        let convo = session.conversations.get(&ConvoId::from("c1")).unwrap();
        assert_eq!(convo.preview.as_deref(), Some("hello there"));
        assert_eq!(
            convo.last_activity,
            DateTime::from_timestamp_millis(9_000).unwrap()
        );
        assert_eq!(convo.message_count, 1);
    }

    #[test]
    fn conversation_deleted_removes_the_row() {
        let mut session = Session::default();
        session.apply(&Update::ConversationsSnapshot(vec![wire("c1", 1_000)]));

        session.apply(&Update::Event(ChatEvent::ConversationDeleted {
            convo_id: logos_chat::ConvoId("c1".to_owned()),
        }));

        assert!(session.conversations.is_empty());
    }

    #[test]
    fn members_loaded_maps_empty_address_to_none_and_flags_self() {
        let mut session = Session::default();
        session.apply(&Update::Ready {
            my_address: "myaddress".to_owned(),
            installation_name: None,
        });
        session.apply(&Update::ConversationsSnapshot(vec![wire("g1", 1_000)]));

        session.apply(&Update::MembersLoaded {
            convo_id: logos_chat::ConvoId("g1".to_owned()),
            members: vec![
                GroupMember {
                    address: "myaddress".to_owned(),
                    pending: false,
                },
                GroupMember {
                    address: String::new(),
                    pending: true,
                },
                GroupMember {
                    address: "peer".to_owned(),
                    pending: false,
                },
            ],
        });

        let members = &session
            .conversations
            .get(&ConvoId::from("g1"))
            .unwrap()
            .members;
        assert_eq!(members.len(), 3);

        assert_eq!(members[0].address, Some(Address::from("myaddress")));
        assert!(members[0].is_self);

        assert_eq!(members[1].address, None);
        assert!(!members[1].is_self);
        assert!(members[1].pending);

        assert_eq!(members[2].address, Some(Address::from("peer")));
        assert!(!members[2].is_self);
    }

    #[test]
    fn ready_recomputes_is_self_for_members_loaded_earlier() {
        // `LoadMembers` is ungated, so members can fold in while identity
        // is still unknown — everyone lands `is_self: false`.
        let mut session = Session::default();
        session.apply(&Update::ConversationsSnapshot(vec![wire("d1", 1_000)]));
        session.apply(&Update::MembersLoaded {
            convo_id: logos_chat::ConvoId("d1".to_owned()),
            members: vec![
                GroupMember {
                    address: "myaddress".to_owned(),
                    pending: false,
                },
                GroupMember {
                    address: "peer".to_owned(),
                    pending: false,
                },
            ],
        });

        let members = |session: &Session| {
            session
                .conversations
                .get(&ConvoId::from("d1"))
                .unwrap()
                .members
                .clone()
        };
        assert!(members(&session).iter().all(|member| !member.is_self));

        session.apply(&Update::Ready {
            my_address: "myaddress".to_owned(),
            installation_name: None,
        });

        let members = members(&session);
        assert!(members[0].is_self);
        assert!(!members[1].is_self);

        // The recompute keeps peer_address() honest for DMs.
        let convo = session.conversations.get(&ConvoId::from("d1")).unwrap();
        assert_eq!(convo.peer_address(), Some(&Address::from("peer")));
    }

    #[test]
    fn phases_and_delivery_events_gate_actions() {
        let mut session = Session::default();
        assert!(!session.delivery.can_act());

        session.apply(&Update::Phase(Phase::Online));
        assert!(session.delivery.can_act());

        session.apply(&Update::Phase(Phase::DeliveryError {
            detail: "boom".to_owned(),
        }));
        assert!(!session.delivery.can_act());
        assert_eq!(session.delivery.detail, "boom");

        session.apply(&Update::Event(ChatEvent::DeliveryStateChanged {
            state: DeliveryState::Online,
            detail: String::new(),
        }));
        assert!(session.delivery.can_act());

        // Unknown wire states keep the previous status.
        session.apply(&Update::Event(ChatEvent::DeliveryStateChanged {
            state: DeliveryState::Unknown,
            detail: "??".to_owned(),
        }));
        assert!(session.delivery.can_act());
    }
}
