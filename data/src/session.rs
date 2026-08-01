//! Pure fold of backend [`logos_chat::Update`]s into domain state — the
//! replacement for the IRC-era `client::Map` message match. History/message
//! integration is wired separately by the UI; only delivery status,
//! identity, and the conversation map live here.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use logos_chat::{ChatEvent, ModuleState, Phase, Update};

use crate::address::Address;
use crate::conversation::{self, Conversation, ConvoId, Kind, Member};
use crate::delivery::{Delivery, DeliveryState, Identity};
use crate::module::{self, Module, ModuleId, Status};

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub delivery: Delivery,
    pub identity: Option<Identity>,
    pub conversations: conversation::Map,
    /// The tracked module set: the daemon itself, then the modules in the
    /// order the backend reports them (which is the staged catalogue's
    /// order). Empty until the first report lands, so the sidebar's module
    /// group renders nothing at all before the daemon has been asked — "no
    /// modules" and "not asked yet" are indistinguishable, and a placeholder
    /// would flicker on every launch.
    pub modules: Vec<Module>,
    /// Modules the log has caught dying.
    ///
    /// Held apart from `modules` because the two facts arrive on different
    /// clocks: the crash is a log line, the status is a 5s poll, and either
    /// can land first. `listModules` never says `crashed` — it reports a
    /// dead module as `not_loaded`, exactly like one that never started —
    /// so without this the pane would go quiet and the row would look idle.
    crashed: HashSet<ModuleId>,
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
            Update::Modules(report) => self.apply_modules(report),
            // Message history is wired by the UI; the remaining variants
            // carry no domain state.
            Update::Controller(_)
            | Update::MessagesLoaded { .. }
            | Update::ActionFailed(_)
            | Update::Fatal(_)
            | Update::Stopped => {}
        }
    }

    pub fn module(&self, id: &ModuleId) -> Option<&Module> {
        self.modules.iter().find(|module| module.id == *id)
    }

    /// Records that `id`'s log announced its death.
    ///
    /// Applied to the row immediately rather than waiting for the next poll:
    /// the abort observed in `logos-modules.md` §5 came ~15s after a `start` that had
    /// returned success, and a row still reading `loaded` while the log has
    /// stopped is the exact confusion the monitor exists to prevent.
    pub fn note_module_crash(&mut self, id: &ModuleId) {
        self.crashed.insert(id.clone());

        if let Some(module) =
            self.modules.iter_mut().find(|module| module.id == *id)
        {
            module.status = Status::Crashed;
        }
    }

    /// Folds one status report onto the staged catalogue.
    ///
    /// The report is authoritative for status, version and membership; the
    /// catalogue supplies what the daemon does not report — declared
    /// dependencies and the derived `protected` flag. A module the daemon
    /// names that we did not stage still gets a row, because hiding it would
    /// make the monitor lie about what is running.
    ///
    /// The daemon leads the list, and is the one row no report can contain:
    /// `listModules` enumerates loadable modules and the daemon is not one.
    /// It writes to the same log as its children though — the untagged lines
    /// the parser attributes to [`ModuleId::daemon`], which is where
    /// `Module process crashed:` and every startup failure appear — so
    /// without a row those lines would accumulate in a history nothing could
    /// open. Answering the poll at all is what makes it `Loaded`, and
    /// `protected` states the obvious: there is no sense in which the process
    /// hosting every module could be unloaded from under them.
    fn apply_modules(&mut self, report: &[ModuleState]) {
        let staged = module::catalog();

        self.modules = std::iter::once(Module {
            id: ModuleId::daemon(),
            status: Status::Loaded,
            version: None,
            dependencies: vec![],
            protected: true,
            recent_events: 0,
        })
        .chain(report.iter().map(|state| {
            let id = ModuleId::from(state.name.as_str());
            let entry = staged.iter().find(|module| module.id == id);
            let reported = Status::from_daemon(&state.status);

            // A load clears the crash: the module is running again, and
            // holding the flag would keep a healthy row red forever.
            if reported.is_loaded() {
                self.crashed.remove(&id);
            }

            let status = if self.crashed.contains(&id) {
                Status::Crashed
            } else {
                reported
            };

            Module {
                status,
                version: state
                    .version
                    .clone()
                    .or_else(|| entry.and_then(|entry| entry.version.clone())),
                dependencies: entry
                    .map(|entry| entry.dependencies.clone())
                    .unwrap_or_default(),
                protected: entry.is_some_and(|entry| entry.protected),
                recent_events: state.recent_events,
                id,
            }
        }))
        .collect();
    }

    fn apply_phase(&mut self, phase: &Phase) {
        // A restart means a fresh daemon and fresh module processes, so a
        // crash learned from the previous one describes something that no
        // longer exists. The backend reseeds its own module set on the same
        // event.
        if matches!(phase, Phase::StartingDaemon | Phase::Restarting { .. }) {
            self.crashed.clear();
        }

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

    fn reported(name: &str, status: &str) -> ModuleState {
        ModuleState {
            name: name.to_owned(),
            status: status.to_owned(),
            version: None,
            recent_events: 0,
        }
    }

    /// The report says what is running; the catalogue says what it is. A
    /// module the daemon reports has to come back carrying the manifest
    /// facts the daemon does not report, or the pane cannot tell the user
    /// that unloading it would take chat down.
    #[test]
    fn a_report_is_folded_onto_the_staged_catalogue() {
        let mut session = Session::default();

        assert!(
            session.modules.is_empty(),
            "no rows before the first report"
        );

        session.apply(&Update::Modules(vec![
            reported("delivery_module", "loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        let delivery =
            session.module(&ModuleId::from("delivery_module")).unwrap();
        assert_eq!(delivery.status, Status::Loaded);
        assert!(delivery.protected, "chat's dependency closure is protected");
        assert_eq!(delivery.version.as_deref(), Some("0.1.3"));

        let blockchain = session
            .module(&ModuleId::from("blockchain_module"))
            .unwrap();
        assert_eq!(blockchain.status, Status::NotLoaded);
        assert!(!blockchain.protected);
    }

    /// The daemon writes to the same log its modules do — its untagged lines
    /// are what `Module process crashed:` and every startup failure arrive
    /// as — and `listModules` will never name it, so the row has to come from
    /// here or those lines pile up in a history with no way to open it.
    #[test]
    fn the_daemon_gets_the_row_its_own_log_needs() {
        let mut session = Session::default();

        assert!(
            session.modules.is_empty(),
            "the daemon has said nothing yet, so neither do we"
        );

        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "loaded",
        )]));

        let daemon = session
            .module(&ModuleId::daemon())
            .expect("the daemon's log has somewhere to be read");

        assert_eq!(
            session.modules.first().map(|module| &module.id),
            Some(&ModuleId::daemon())
        );
        assert_eq!(
            daemon.status,
            Status::Loaded,
            "it answered the poll we are folding",
        );
        assert!(daemon.protected, "the host process is not unloadable");
        assert_eq!(daemon.display_name(), "Logoscore");
    }

    /// Findings §5: a module aborts and the very next poll reports it
    /// `not_loaded`, which is what an idle module reports too. The log is
    /// the only place the difference exists, so the flag has to survive
    /// every subsequent report until the module is loaded again.
    #[test]
    fn a_crash_outlives_the_polls_that_call_it_merely_not_loaded() {
        let mut session = Session::default();
        let id = ModuleId::from("blockchain_module");

        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "loaded",
        )]));
        assert_eq!(session.module(&id).unwrap().status, Status::Loaded);

        session.note_module_crash(&id);
        assert_eq!(
            session.module(&id).unwrap().status,
            Status::Crashed,
            "the row must not wait 5s for the poll to catch up",
        );

        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "not_loaded",
        )]));
        assert_eq!(session.module(&id).unwrap().status, Status::Crashed);

        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "loaded",
        )]));
        assert_eq!(
            session.module(&id).unwrap().status,
            Status::Loaded,
            "a module that loaded again is not still crashed",
        );
    }

    /// The crash line can beat the poll that would have created the row —
    /// the log tail runs on its own subscription and does not wait for the
    /// backend. The flag has to be remembered, not dropped on the floor.
    #[test]
    fn a_crash_seen_before_the_first_report_still_lands() {
        let mut session = Session::default();
        let id = ModuleId::from("blockchain_module");

        session.note_module_crash(&id);

        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "not_loaded",
        )]));

        assert_eq!(session.module(&id).unwrap().status, Status::Crashed);
    }

    /// A restart hands us a fresh daemon and fresh module processes, so a
    /// crash held from the previous one describes a process that no longer
    /// exists.
    #[test]
    fn restarting_the_stack_forgets_the_previous_run_crash() {
        let mut session = Session::default();
        let id = ModuleId::from("blockchain_module");

        session.note_module_crash(&id);
        session.apply(&Update::Phase(Phase::StartingDaemon));
        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "not_loaded",
        )]));

        assert_eq!(session.module(&id).unwrap().status, Status::NotLoaded);
    }

    /// The daemon is the authority on what is running. A module we never
    /// staged still gets a row rather than being hidden, because a monitor
    /// that omits a running process is worse than one that shows an
    /// unfamiliar name.
    #[test]
    fn an_unstaged_module_the_daemon_reports_still_gets_a_row() {
        let mut session = Session::default();

        session.apply(&Update::Modules(vec![reported(
            "mystery_module",
            "loaded",
        )]));

        let mystery =
            session.module(&ModuleId::from("mystery_module")).unwrap();
        assert_eq!(mystery.status, Status::Loaded);
        assert!(mystery.dependencies.is_empty());
        assert!(!mystery.protected);
    }
}
