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

/// How many reports a ladder may go on claiming to be running for.
///
/// The bound exists because the phase that ends a ladder — `Online` — is
/// published by the delivery module, not by the ladder, and a delivery that
/// never comes online never publishes it. Without a bound the rows the
/// ladder claimed would read "loading" for the rest of the run, which is the
/// original complaint ("stuck at not-loaded") wearing a nicer word.
///
/// It is counted in reports rather than seconds because this is a pure fold
/// with no clock, and because reports are the signal that says what the
/// bound is really about: the backend polls modules only from its main loop,
/// so a report arriving is evidence the ladder has handed off. One report is
/// expected during a ladder (the backend reseeds its own set on the way
/// down); everything past that is the main loop running. Three ≈ 15s at the
/// 5s poll, i.e. two polls of slack past the reseed before we stop promising.
const LADDER_GRACE_POLLS: u32 = 3;

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
    /// How many times the backend has restarted under this run.
    ///
    /// A restart is invisible by construction: it takes about four seconds,
    /// the daemon log is truncated when the new process opens it, and the
    /// fresh daemon reports a perfectly ordinary module set — so a user who
    /// blinked has no way to tell that everything they were watching died
    /// and came back. The count is the trace, and it is a count rather than
    /// a flag because the second restart in a minute means something the
    /// first does not.
    pub restarts: u32,
    /// Modules the log has caught dying, plus those a report caught
    /// vanishing.
    ///
    /// Held apart from `modules` because the two facts arrive on different
    /// clocks: the crash is a log line, the status is a 5s poll, and either
    /// can land first. `listModules` never says `crashed` — it reports a
    /// dead module as `not_loaded`, exactly like one that never started —
    /// so without this the pane would go quiet and the row would look idle.
    ///
    /// It deliberately outlives a restart. The crash that *caused* the
    /// restart is the one whose evidence the restart destroys, so forgetting
    /// it here is forgetting it everywhere; a module that comes back clears
    /// its own flag by loading.
    crashed: HashSet<ModuleId>,
    /// Modules **the app is loading right now**.
    ///
    /// This is the whole warrant for [`Status::Loading`], and it is why the
    /// state is never guessed from a poll: the set is chat's dependency
    /// closure and nothing else, because `load_module(chat_module)` is the
    /// only load the app performs. A module an operator loaded by hand is
    /// never in it — nobody is loading that, so calling it "loading" would
    /// be a plausible lie about work no one is doing.
    ///
    /// Membership is granted by the ladder and resolved by the first report
    /// after the ladder ends. Membership alone is *not* evidence of death:
    /// a module that was asked to load and never appeared failed to load; it
    /// did not die. That inference belongs to [`Session::carried`].
    loading: HashSet<ModuleId>,
    /// Modules that **were running when a ladder tore the stack down**.
    ///
    /// Held apart from [`Session::loading`] because it answers a different
    /// question and is used for exactly one thing: if such a module does not
    /// come back, it died. It is never rendered — a module nothing is
    /// loading may not read "loading" merely because it used to be up — so
    /// the operator-loaded module that crashed and took the daemon with it
    /// ends up `Crashed` without ever having claimed to be in flight.
    ///
    /// It is equally where a death *seen during* a ladder waits, since the
    /// question is the same one: was it running, and did it come back? That
    /// is what lets the fold defer such a verdict instead of discarding it.
    carried: HashSet<ModuleId>,
    /// Whether the backend's startup ladder is running.
    ///
    /// "The ladder is running" and nothing else: see [`Session::apply_phase`]
    /// for which phases may say so. While it holds, a report describes a
    /// stack that is still being assembled, so a module missing from it is
    /// the rebuild talking rather than a death.
    starting: bool,
    /// Reports folded since the current ladder began.
    ///
    /// Bounds the claim a stalled ladder would otherwise make forever: the
    /// `Loading` rows, via [`LADDER_GRACE_POLLS`].
    ladder_polls: u32,
    /// Whether anything has proved the daemon is answering since the last
    /// time we were told it is being started.
    ///
    /// The daemon's own row is the one row no report contains, so it is
    /// inferred — and the only report that is *not* evidence of a live
    /// daemon is the one the backend publishes on its own behalf while the
    /// process is down (`reset_modules`, on the way into a ladder). Which
    /// report that is cannot be read off an arrival count: the restart path
    /// publishes it between `Restarting` and `StartingDaemon`, the first-boot
    /// path publishes it before the ladder opens at all. So the question is
    /// asked of the evidence instead: the last rung is emitted from the far
    /// side of a round trip the daemon served, and a second report can only
    /// have come through the poll — either settles it, and both land before
    /// the first poll of either path.
    daemon_live: bool,
}

/// Chat's dependency closure: `chat_module` plus the `delivery_module` the
/// daemon auto-loads with it.
///
/// The only modules the app itself ever loads, and therefore the only ones
/// it may honestly call [`Status::Loading`].
fn chat_closure() -> impl Iterator<Item = ModuleId> {
    module::catalog()
        .into_iter()
        .filter(|module| module.protected)
        .map(|module| module.id)
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
    ///
    /// The daemon's own row reads `Loading` for the backend's reseed and for
    /// nothing else. The reseed is a set the backend published on its own
    /// behalf while the daemon was down, so calling the process we are still
    /// starting `Loaded` is the one claim this row cannot make. Every other
    /// report reached us through the module poll, which only a live daemon
    /// can answer — so a ladder that stalls short of `Online` must not go on
    /// calling the process that is demonstrably serving us "loading". Which
    /// report is the reseed is decided by [`Session::daemon_live`] rather
    /// than by counting arrivals, because the two startup paths publish it at
    /// different points in the phase stream.
    fn apply_modules(&mut self, report: &[ModuleState]) {
        let staged = module::catalog();
        let previous = std::mem::take(&mut self.modules);

        // A report is the backend's main loop talking, and the main loop
        // does not run while a ladder does. Past the reseed, one arriving
        // with the ladder still unfinished means the ladder stalled — so the
        // claims it made are withdrawn rather than left standing.
        if self.starting {
            self.ladder_polls += 1;
            self.starting = self.ladder_polls <= LADDER_GRACE_POLLS;
        }

        let daemon = if self.starting && !self.daemon_live {
            Status::Loading
        } else {
            Status::Loaded
        };

        // The backend publishes one set per teardown on its own behalf, so
        // whatever this report was, a further one cannot be that seed: it
        // came through the poll, and a poll is served by a live daemon.
        self.daemon_live = true;

        self.modules = std::iter::once(Module {
            id: ModuleId::daemon(),
            status: daemon,
            version: None,
            dependencies: vec![],
            protected: true,
            recent_events: 0,
        })
        .chain(report.iter().map(|state| {
            let id = ModuleId::from(state.name.as_str());
            let entry = staged.iter().find(|module| module.id == id);
            let status = self.resolve_status(
                &id,
                Status::from_daemon(&state.status),
                previous
                    .iter()
                    .find(|module| module.id == id)
                    .map(|module| &module.status),
            );

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

    /// What one reported module is really doing, given what it was doing a
    /// moment ago and whether the stack is mid-restart.
    ///
    /// The rule the daemon cannot express: a module we saw `Loaded` that
    /// comes back `not_loaded` **died**. Nothing else can move it — the app
    /// has no unload control, so there is no benign path from running to
    /// gone — and inferring it here is what keeps a crash visible when the
    /// respawn has truncated the log the `critical` line was on.
    ///
    /// The restart is the one exception, and it is an exception we *know*
    /// about rather than guess at: while the ladder runs, every module
    /// legitimately goes away at once, so nothing is called dead for it.
    /// What was up is remembered in [`Session::carried`], and the first
    /// report after the ladder finishes decides which of them came back.
    /// That is how a module whose crash took the daemon with it ends up
    /// crashed rather than merely absent, without painting the sidebar red
    /// for the modules that returned.
    ///
    /// It is an exception that *defers*, never one that discards. A ladder's
    /// window outlasts its rungs — it ends at `Online`, which delivery
    /// publishes when it is ready to, so the main loop can already be polling
    /// while the flag still holds — and a module seen running and then gone
    /// inside that window is evidence just the same. Dropping it would lose
    /// the crash for good, because the row the rule reads next time has by
    /// then been rewritten to the absence it was supposed to explain.
    ///
    /// The evidence of death is *having been up*, never *having been asked
    /// to come up*. A module the ladder loads that never appears has failed
    /// to load, which is a different fact from having died, and is reported
    /// as the daemon reports it: `not loaded`.
    fn resolve_status(
        &mut self,
        id: &ModuleId,
        reported: Status,
        previous: Option<&Status>,
    ) -> Status {
        // A load clears the crash: the module is running again, and holding
        // the flag would keep a healthy row red forever.
        if reported.is_loaded() {
            self.crashed.remove(id);
            self.loading.remove(id);
            self.carried.remove(id);

            return reported;
        }

        if self.crashed.contains(id) {
            return Status::Crashed;
        }

        if self.starting {
            // Nothing is *decided* while the stack is being assembled — but
            // nothing is thrown away either. A module we watched running that
            // a report now calls gone is the same evidence it always was;
            // the ladder is only a reason to wait and see whether it comes
            // back, which is exactly what `carried` remembers.
            if reported == Status::NotLoaded
                && previous == Some(&Status::Loaded)
            {
                self.carried.insert(id.clone());
            }

            // The rows the app is loading say so; everything else is passed
            // through exactly as the daemon worded it.
            return if self.loading.contains(id) {
                Status::Loading
            } else {
                reported
            };
        }

        self.loading.remove(id);

        let was_running =
            self.carried.remove(id) || previous == Some(&Status::Loaded);

        if reported == Status::NotLoaded && was_running {
            self.crashed.insert(id.clone());

            return Status::Crashed;
        }

        reported
    }

    /// Opens a ladder: notes what was running so its absence can be read
    /// later, and claims the closure the ladder is about to load.
    ///
    /// The rows are rewritten immediately rather than waiting for the
    /// backend's reseeded report, which arrives a moment later and says
    /// `not_loaded` for everything: between the two, a row reading `loaded`
    /// describes a process that has already been killed. What the app is
    /// reloading reads `Loading`; what it is not reads `NotLoaded`, because
    /// nothing is loading it and the process really is gone.
    ///
    /// It also withdraws the daemon's proof of life: whatever we knew about
    /// the process, the ladder is about to start a different one.
    fn begin_ladder(&mut self) {
        let daemon = ModuleId::daemon();

        self.starting = true;
        self.ladder_polls = 0;
        self.daemon_live = false;
        self.loading = chat_closure().collect();

        for module in &mut self.modules {
            if module.id == daemon {
                module.status = Status::Loading;

                continue;
            }

            if module.status.is_loaded() {
                self.carried.insert(module.id.clone());
                module.status = Status::NotLoaded;
            }

            if self.loading.contains(&module.id) {
                module.status = Status::Loading;
            }
        }
    }

    /// Closes a ladder that reached a verdict. The sets survive: the first
    /// report after this is the one that decides what came back.
    fn end_ladder(&mut self) {
        self.starting = false;
        self.ladder_polls = 0;
    }

    /// The ladder exhausted its attempts. Nothing is coming back, and no
    /// further report will arrive to correct a row — so the inference has to
    /// be settled here or it is lost in the one case where it matters most.
    ///
    /// A module that was running when the backend went down and has now been
    /// given up on is dead, and says so. A module that was merely being
    /// loaded never ran, so it is not called dead: it failed to load, and
    /// reads as the daemon would report it.
    fn abandon_ladder(&mut self) {
        self.starting = false;
        self.ladder_polls = 0;
        self.loading.clear();

        let carried = std::mem::take(&mut self.carried);

        for module in &mut self.modules {
            if carried.contains(&module.id) {
                module.status = Status::Crashed;
            } else if module.status == Status::Loading {
                module.status = Status::NotLoaded;
            }
        }

        self.crashed.extend(carried);
    }

    /// We are quitting. A clean stop kills every module by design, so
    /// nothing here is evidence of anything and no claim outlives it.
    fn stop_ladder(&mut self) {
        self.starting = false;
        self.ladder_polls = 0;
        self.loading.clear();
        self.carried.clear();

        for module in &mut self.modules {
            if module.status == Status::Loading {
                module.status = Status::NotLoaded;
            }
        }
    }

    /// Folds one backend phase.
    ///
    /// Which phases mean "a stack is being assembled" is the load-bearing
    /// question here, and it is not answerable from the variant names — it
    /// depends on where `logos-chat` emits each one. Read off
    /// `logos/chat/src/session.rs`:
    ///
    /// | Phase | emitted from | mid-run, outside a ladder? |
    /// |---|---|---|
    /// | `StartingDaemon` | `start_stack` | no — it *is* the first rung |
    /// | `Connecting` | `connect_stack` | no |
    /// | `LoadingModule` | `connect_stack` | no |
    /// | `InitialisingChat` | `connect_stack` **and** `handle_delivery` | **yes** |
    /// | `Online` | `handle_delivery` | yes |
    /// | `DeliveryError` | `handle_delivery` | yes |
    /// | `DeliveryStopped` | `handle_delivery` | yes |
    /// | `Restarting` | the restart loop | no — it *is* the first rung |
    /// | `Failed` | `fatal`, terminal | no |
    /// | `ShuttingDown` | `quit`, terminal | no |
    ///
    /// `handle_delivery` runs from the main loop's event pump and from the
    /// `chat.status()` seed at the end of the ladder, so every phase it
    /// emits means the ladder is no longer climbing. Three of those are safe
    /// verdicts. The fourth, `InitialisingChat`, is the trap: it is emitted
    /// once as a rung and again on **any** live `delivery_state_changed`
    /// carrying `initialising`, which a healthy backend does whenever
    /// delivery blips. It is therefore a *continuation* inside a ladder and a
    /// *live event* outside one, and may neither set nor clear the ladder
    /// flag — treating it as a ladder is what swallowed real crashes and made
    /// the live daemon's own row read "loading". What it does settle is
    /// narrower and never in doubt: the daemon is answering.
    fn apply_phase(&mut self, phase: &Phase) {
        match phase {
            // A restart tears down the daemon and every module process with
            // it. `StartingDaemon` is included because it is also the first
            // phase of a fresh run, where there is nothing to carry.
            Phase::StartingDaemon | Phase::Restarting { .. } => {
                self.begin_ladder();
            }
            // Rungs. They cannot open a ladder — there is nothing to carry
            // by the time they arrive — but they do prove one is climbing.
            Phase::Connecting => self.starting = true,
            // The rung at which the app performs its one load.
            Phase::LoadingModule => {
                self.starting = true;
                self.loading.extend(chat_closure());
            }
            Phase::Online
            | Phase::DeliveryError { .. }
            | Phase::DeliveryStopped => self.end_ladder(),
            Phase::Failed => self.abandon_ladder(),
            Phase::ShuttingDown => self.stop_ladder(),
            // Ambiguous about the ladder by construction; see the table
            // above. It is not ambiguous about the daemon: both emitters sit
            // behind a round trip the daemon served — `connect_stack`
            // publishes it having already loaded a module over the link, and
            // a live `delivery_state_changed` had to arrive from somewhere.
            // It is also the last rung, so on either startup path it lands
            // before the main loop can poll: whatever reports follow, none of
            // them is the seed the backend published while the daemon was
            // down.
            Phase::InitialisingChat => self.daemon_live = true,
        }

        // Only the first attempt of an episode is a restart; the ones after
        // it are this restart failing to take, and the status bar would be
        // counting its own retries.
        if let Phase::Restarting { attempt } = phase
            && *attempt == 1
        {
            self.restarts += 1;
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

    /// The motivating session, in miniature: the module that aborted took
    /// the daemon with it, the respawn truncated the log the crash was
    /// written to, and the fresh daemon reports an unremarkable
    /// `not_loaded`. If the restart clears the flag, the only surviving
    /// evidence of the crash is gone — which is precisely what made a
    /// crashed module read as one that was never started.
    #[test]
    fn a_crash_outlives_the_restart_it_caused() {
        let mut session = Session::default();
        let id = ModuleId::from("blockchain_module");

        session.note_module_crash(&id);
        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));
        session.apply(&Update::Phase(Phase::StartingDaemon));
        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "not_loaded",
        )]));

        assert_eq!(session.module(&id).unwrap().status, Status::Crashed);

        // Loading it again is the only thing that clears it, restart or no
        // restart: the module is running, so the row must not stay red.
        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "loaded",
        )]));

        assert_eq!(session.module(&id).unwrap().status, Status::Loaded);
    }

    /// The rule the daemon's two-word vocabulary cannot state: a module we
    /// watched run, which the next poll calls `not_loaded`, died. The app
    /// has no unload control, so there is no innocent way to make that
    /// transition — and it is the one piece of crash evidence a truncated
    /// log cannot destroy.
    #[test]
    fn a_module_that_stops_being_loaded_on_its_own_is_a_crash() {
        let mut session = Session::default();
        let id = ModuleId::from("blockchain_module");

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "loaded",
        )]));
        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "not_loaded",
        )]));

        assert_eq!(session.module(&id).unwrap().status, Status::Crashed);
    }

    /// A restart is the one time every module legitimately goes away at
    /// once, and it is a *known* event rather than an inference — so it must
    /// not be read as four simultaneous crashes. What was running is held as
    /// coming back; what was idle stays idle and is claimed for nothing.
    #[test]
    fn a_restart_holds_what_was_running_instead_of_burying_it() {
        let mut session = Session::default();
        let chat = ModuleId::from("chat_module");
        let blockchain = ModuleId::from("blockchain_module");

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));

        assert_eq!(
            session.module(&chat).unwrap().status,
            Status::Loading,
            "a row still reading loaded describes a killed process",
        );
        assert_eq!(
            session.module(&ModuleId::daemon()).unwrap().status,
            Status::Loading,
            "the daemon is the process being restarted",
        );

        // The backend reseeds its own set on the way back up, and says
        // `not_loaded` for everything. None of that is a death.
        session.apply(&Update::Phase(Phase::StartingDaemon));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "not_loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        assert_eq!(session.module(&chat).unwrap().status, Status::Loading);
        assert_eq!(
            session.module(&blockchain).unwrap().status,
            Status::NotLoaded,
            "a module that was not running is not coming back either",
        );

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        assert_eq!(session.module(&chat).unwrap().status, Status::Loaded);
        assert_eq!(
            session.module(&blockchain).unwrap().status,
            Status::NotLoaded,
        );
        assert_eq!(session.restarts, 1);
    }

    /// The other half of the restart rule. Everything went away together,
    /// but only one of them failed to come back — and "absent after a
    /// restart it caused" is the same fact as "crashed", stated by the one
    /// signal the truncation left us.
    #[test]
    fn a_module_that_does_not_come_back_from_a_restart_reads_crashed() {
        let mut session = Session::default();
        let chat = ModuleId::from("chat_module");
        let blockchain = ModuleId::from("blockchain_module");

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "loaded"),
        ]));

        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));
        session.apply(&Update::Phase(Phase::StartingDaemon));
        session.apply(&Update::Phase(Phase::LoadingModule));
        session.apply(&Update::Phase(Phase::Online));

        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        assert_eq!(session.module(&chat).unwrap().status, Status::Loaded);
        assert_eq!(
            session.module(&blockchain).unwrap().status,
            Status::Crashed,
        );
    }

    /// Honest ignorance beats a plausible lie. A module an operator loads by
    /// hand is only ever seen after the fact, in a poll — we did not watch
    /// it start, so it goes straight from idle to running and is never
    /// dressed up as something we were doing.
    #[test]
    fn a_module_loaded_behind_our_back_is_never_called_loading() {
        let mut session = Session::default();
        let id = ModuleId::from("blockchain_module");

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "not_loaded",
        )]));
        assert_eq!(session.module(&id).unwrap().status, Status::NotLoaded);

        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "loaded",
        )]));
        assert_eq!(session.module(&id).unwrap().status, Status::Loaded);
    }

    /// The ladder loads chat itself, so that is the one module set a first
    /// run may honestly call `Loading` — chat and the dependency the daemon
    /// auto-loads with it, and nothing else.
    #[test]
    fn the_module_the_ladder_loads_reads_loading_while_it_does() {
        let mut session = Session::default();

        session.apply(&Update::Phase(Phase::StartingDaemon));
        session.apply(&Update::Phase(Phase::LoadingModule));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "not_loaded"),
            reported("delivery_module", "not_loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        assert_eq!(
            session
                .modules
                .iter()
                .map(|module| (module.id.to_string(), module.status.label()))
                .collect::<Vec<_>>(),
            vec![
                ("logoscore".to_owned(), "loading"),
                ("chat_module".to_owned(), "loading"),
                ("delivery_module".to_owned(), "loading"),
                ("blockchain_module".to_owned(), "not loaded"),
            ],
        );
    }

    /// A ladder that gave up is not a load in flight. There will be no
    /// further report to correct the row, so the promise has to be withdrawn
    /// here rather than left on screen for the rest of the run.
    #[test]
    fn a_failed_backend_stops_claiming_anything_is_loading() {
        let mut session = Session::default();
        let id = ModuleId::from("chat_module");

        session.apply(&Update::Phase(Phase::LoadingModule));
        session.apply(&Update::Modules(vec![reported(
            "chat_module",
            "not_loaded",
        )]));
        assert_eq!(session.module(&id).unwrap().status, Status::Loading);

        session.apply(&Update::Phase(Phase::Failed));

        assert_eq!(session.module(&id).unwrap().status, Status::NotLoaded);
    }

    /// The restart count is the only trace a recovered restart leaves, and
    /// it counts episodes rather than the ladder's own retries — otherwise
    /// one backend death that took three attempts to recover from would
    /// report itself three times.
    #[test]
    fn the_restart_count_counts_episodes_not_attempts() {
        let mut session = Session::default();

        assert_eq!(session.restarts, 0);

        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));
        session.apply(&Update::Phase(Phase::Restarting { attempt: 2 }));
        session.apply(&Update::Phase(Phase::Restarting { attempt: 3 }));
        assert_eq!(session.restarts, 1);

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));
        assert_eq!(session.restarts, 2);
    }

    /// `Phase::InitialisingChat` is not a ladder. `logos-chat` re-emits it
    /// for every live `delivery_state_changed("initialising")`, which a
    /// healthy backend does whenever delivery blips — so reading it as "the
    /// stack is being assembled" hands the whole crash inference an
    /// off-switch that an ordinary wire event can flip. A module dying
    /// during the blip was swallowed as merely `not_loaded`, and because
    /// that rewrote its `previous`, the `Loaded -> NotLoaded` rule could
    /// never fire for it again: the crash was lost for the rest of the run.
    #[test]
    fn a_live_delivery_blip_does_not_swallow_a_crash() {
        let mut session = Session::default();
        let blockchain = ModuleId::from("blockchain_module");

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "loaded"),
        ]));

        // No restart, no daemon death: delivery re-entered initialising and
        // the backend said so the only way it can.
        session.apply(&Update::Phase(Phase::InitialisingChat));

        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        assert_eq!(
            session.module(&blockchain).unwrap().status,
            Status::Crashed,
            "a delivery blip was read as the stack being rebuilt",
        );

        // And it survives delivery recovering, which is where the old rule
        // lost it for good.
        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        assert_eq!(
            session.module(&blockchain).unwrap().status,
            Status::Crashed,
        );
    }

    /// The row a user reads as "is the backend up" may not be fabricated.
    /// The daemon answered the poll being folded, so it is up — and while
    /// `InitialisingChat` counted as a ladder, an ordinary delivery blip put
    /// the live daemon's own row into "loading".
    #[test]
    fn the_daemon_that_answered_the_poll_is_not_still_starting() {
        let mut session = Session::default();

        session.apply(&Update::Phase(Phase::Online));
        session
            .apply(&Update::Modules(vec![reported("chat_module", "loaded")]));
        session.apply(&Update::Phase(Phase::InitialisingChat));
        session
            .apply(&Update::Modules(vec![reported("chat_module", "loaded")]));

        assert_eq!(
            session.module(&ModuleId::daemon()).unwrap().status,
            Status::Loaded,
            "the daemon served the very poll this row was built from",
        );
    }

    /// The daemon's row is `Loading` for the one report that is not a daemon
    /// speaking — the backend reseeding its staged set on the way down —
    /// and `Loaded` for every report after it, because a report past the
    /// reseed came through the module poll and only a live daemon answers
    /// those. A ladder that stalls therefore cannot go on calling the
    /// process that is serving us "loading".
    #[test]
    fn only_the_backends_own_reseed_leaves_the_daemon_row_loading() {
        let mut session = Session::default();

        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));
        session.apply(&Update::Modules(vec![reported(
            "chat_module",
            "not_loaded",
        )]));

        assert_eq!(
            session.module(&ModuleId::daemon()).unwrap().status,
            Status::Loading,
            "the reseed is published while the daemon is down",
        );

        session.apply(&Update::Modules(vec![reported(
            "chat_module",
            "not_loaded",
        )]));

        assert_eq!(
            session.module(&ModuleId::daemon()).unwrap().status,
            Status::Loaded,
            "a poll answered mid-ladder is a daemon that is up",
        );
    }

    /// Whether a report is the backend's own seed or a daemon answering
    /// cannot be read off its arrival number, because the two startup paths
    /// publish the seed at different points in the phase stream: first boot
    /// emits it before the ladder opens at all, a restart emits it between
    /// `Restarting` and `StartingDaemon`. A count reset by the ladder is
    /// therefore spent on a poll a live daemon served, and the row reads
    /// "loading" for a process that just answered us.
    #[test]
    fn neither_startup_path_calls_a_poll_the_daemon_answered_loading() {
        let daemon = |session: &Session| {
            session.module(&ModuleId::daemon()).unwrap().status.clone()
        };

        // First boot: `start_stack` reseeds before it says a word.
        let mut session = Session::default();
        session.apply(&Update::Modules(vec![reported(
            "chat_module",
            "not_loaded",
        )]));
        session.apply(&Update::Phase(Phase::StartingDaemon));

        assert_eq!(
            daemon(&session),
            Status::Loading,
            "the process the ladder is starting is not up yet",
        );

        session.apply(&Update::Phase(Phase::Connecting));
        session.apply(&Update::Phase(Phase::LoadingModule));
        // Delivery is still syncing, so the seed at the end of the ladder
        // publishes this instead of `Online` and the flag stays up.
        session.apply(&Update::Phase(Phase::InitialisingChat));
        session
            .apply(&Update::Modules(vec![reported("chat_module", "loaded")]));

        assert_eq!(
            daemon(&session),
            Status::Loaded,
            "that report came through the poll, which only a live daemon \
             answers",
        );

        // Restart: the reseed lands inside the ladder this time.
        let mut session = Session::default();
        session.apply(&Update::Phase(Phase::Online));
        session
            .apply(&Update::Modules(vec![reported("chat_module", "loaded")]));
        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));
        session.apply(&Update::Modules(vec![reported(
            "chat_module",
            "not_loaded",
        )]));

        assert_eq!(
            daemon(&session),
            Status::Loading,
            "the reseed is published while the daemon is down",
        );

        session.apply(&Update::Phase(Phase::StartingDaemon));
        session.apply(&Update::Phase(Phase::Connecting));
        session.apply(&Update::Phase(Phase::LoadingModule));
        session.apply(&Update::Phase(Phase::InitialisingChat));
        session
            .apply(&Update::Modules(vec![reported("chat_module", "loaded")]));

        assert_eq!(
            daemon(&session),
            Status::Loaded,
            "the ladder's own reset spent the seed allowance on a real poll",
        );
    }

    /// A ladder's window outlasts its rungs: `connect_stack` ends by seeding
    /// `chat.status()`, and a delivery that has not finished syncing answers
    /// `InitialisingChat` — no verdict at all — so the flag stays up while
    /// the main loop is already polling every 5s. A module that dies in
    /// there used to be discarded: the raw `not loaded` was rendered and
    /// forgotten, and because that rewrote the row the rule reads, the crash
    /// could never be re-derived afterwards. The ladder is a reason to defer
    /// the verdict, never to drop the evidence.
    #[test]
    fn a_crash_inside_the_ladder_window_is_deferred_not_discarded() {
        let mut session = Session::default();
        let blockchain = ModuleId::from("blockchain_module");

        session.apply(&Update::Phase(Phase::StartingDaemon));
        session.apply(&Update::Phase(Phase::Connecting));
        session.apply(&Update::Phase(Phase::LoadingModule));
        session.apply(&Update::Phase(Phase::InitialisingChat));

        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "loaded"),
        ]));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        assert_eq!(
            session.module(&blockchain).unwrap().status,
            Status::Crashed,
            "a module was watched dying and the ladder swallowed it",
        );
        assert_eq!(
            session
                .module(&ModuleId::from("chat_module"))
                .unwrap()
                .status,
            Status::Loaded,
        );
    }

    /// The guard the deferral must not trample. Everything going away at
    /// once is what a restart *is*, so a stack that comes back whole is not
    /// four deaths — including when the modules blink through the ladder's
    /// window, which is the same window a real crash is now deferred into.
    #[test]
    fn a_restart_that_returns_whole_leaves_nothing_looking_crashed() {
        let mut session = Session::default();

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "loaded"),
        ]));

        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));
        session.apply(&Update::Phase(Phase::StartingDaemon));
        session.apply(&Update::Phase(Phase::Connecting));
        session.apply(&Update::Phase(Phase::LoadingModule));
        session.apply(&Update::Phase(Phase::InitialisingChat));

        // Still inside the window: the daemon is up and answering, and the
        // modules are coming back one poll behind it.
        session.apply(&Update::Modules(vec![
            reported("chat_module", "not_loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "loaded"),
        ]));

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "loaded"),
        ]));

        assert!(
            session
                .modules
                .iter()
                .all(|module| module.status != Status::Crashed),
            "an ordinary restart was read as the modules dying in it",
        );
        assert!(
            session.crashed.is_empty(),
            "and nothing is remembered as dead"
        );
    }

    /// `Loading` means *the app is loading this*, and the app loads exactly
    /// chat's closure. A module an operator loaded by hand is carried
    /// through a restart only so its absence can be read afterwards — it is
    /// never dressed up as work in flight, because nothing will ever reload
    /// it and the promise could not be kept.
    #[test]
    fn a_module_nothing_is_loading_never_reads_loading_through_a_restart() {
        let mut session = Session::default();
        let blockchain = ModuleId::from("blockchain_module");

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "loaded"),
        ]));

        let ladder = [
            Phase::Restarting { attempt: 1 },
            Phase::StartingDaemon,
            Phase::Connecting,
            Phase::LoadingModule,
            Phase::InitialisingChat,
        ];

        for phase in ladder {
            session.apply(&Update::Phase(phase));

            assert_ne!(
                session.module(&blockchain).unwrap().status,
                Status::Loading,
                "claimed to be loading a module nothing loads",
            );
        }

        session.apply(&Update::Modules(vec![
            reported("chat_module", "not_loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        assert_eq!(
            session
                .module(&ModuleId::from("chat_module"))
                .unwrap()
                .status,
            Status::Loading,
            "the ladder really is loading chat",
        );
        assert_ne!(
            session.module(&blockchain).unwrap().status,
            Status::Loading,
        );

        // Carrying it was still the point: it was up, it did not come back.
        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("blockchain_module", "not_loaded"),
        ]));

        assert_eq!(
            session.module(&blockchain).unwrap().status,
            Status::Crashed,
        );
    }

    /// A promise of "not yet" that is never resolved is the original
    /// complaint with a nicer word on it. `Online` is published by the
    /// delivery module rather than by the ladder, so a delivery that never
    /// comes online never ends the ladder — and the rows it claimed would
    /// read "loading" for the rest of the run. Reports are the bound: the
    /// backend polls modules only from its main loop, so one arriving past
    /// the reseed says the ladder has handed off and stalled.
    #[test]
    fn a_stalled_ladder_stops_claiming_a_load_is_in_flight() {
        let mut session = Session::default();
        let chat = ModuleId::from("chat_module");

        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));
        session.apply(&Update::Phase(Phase::StartingDaemon));
        session.apply(&Update::Phase(Phase::Connecting));
        session.apply(&Update::Phase(Phase::LoadingModule));
        // The last phase this run will ever emit: delivery never reaches
        // online and the backend never gives up either.
        session.apply(&Update::Phase(Phase::InitialisingChat));

        for _ in 0..50 {
            session.apply(&Update::Modules(vec![reported(
                "chat_module",
                "not_loaded",
            )]));
        }

        assert_eq!(
            session.module(&chat).unwrap().status,
            Status::NotLoaded,
            "a load nobody is making was promised forever",
        );
        assert_eq!(
            session.module(&ModuleId::daemon()).unwrap().status,
            Status::Loaded,
            "the daemon answered every one of those polls",
        );
    }

    /// The grace is real, though: a ladder is allowed to be mid-flight while
    /// the backend reseeds and polls again, and withdrawing the claim on the
    /// first report would make `Loading` almost unobservable.
    #[test]
    fn the_ladders_claim_survives_the_reseed_that_follows_it() {
        let mut session = Session::default();
        let chat = ModuleId::from("chat_module");

        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));
        session.apply(&Update::Modules(vec![reported(
            "chat_module",
            "not_loaded",
        )]));

        assert_eq!(session.module(&chat).unwrap().status, Status::Loading);

        session.apply(&Update::Phase(Phase::StartingDaemon));
        session.apply(&Update::Phase(Phase::LoadingModule));
        session.apply(&Update::Modules(vec![reported(
            "chat_module",
            "not_loaded",
        )]));

        assert_eq!(session.module(&chat).unwrap().status, Status::Loading);
    }

    /// "Never came up" is not "died". A module the ladder was asked to load
    /// and which never appeared has failed to load — claiming it crashed is
    /// a statement about a process that never ran, and it would paint the
    /// row red, glyph and crash banner included, on a healthy startup where
    /// the daemon's dependency auto-load was one poll behind.
    #[test]
    fn a_module_that_never_came_up_did_not_crash() {
        let mut session = Session::default();
        let delivery = ModuleId::from("delivery_module");

        session.apply(&Update::Phase(Phase::StartingDaemon));
        session.apply(&Update::Phase(Phase::Connecting));
        session.apply(&Update::Phase(Phase::LoadingModule));
        session.apply(&Update::Phase(Phase::InitialisingChat));
        session.apply(&Update::Phase(Phase::Online));

        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("delivery_module", "not_loaded"),
        ]));

        assert_eq!(
            session.module(&delivery).unwrap().status,
            Status::NotLoaded,
            "a module that never ran was reported as having died",
        );

        // And it is not remembered as a crash either — the flag, once set,
        // only a `loaded` report clears.
        session.apply(&Update::Modules(vec![
            reported("chat_module", "loaded"),
            reported("delivery_module", "not_loaded"),
        ]));

        assert_eq!(
            session.module(&delivery).unwrap().status,
            Status::NotLoaded
        );
    }

    /// The one case where the inference has nowhere else to run. When the
    /// ladder exhausts its attempts there will be no further report, so a
    /// module that was running when the backend went down has to be settled
    /// here — and rewriting it to `not loaded` discards the crash in exactly
    /// the situation where nothing is coming back to re-derive it.
    #[test]
    fn an_exhausted_ladder_leaves_what_died_looking_dead() {
        let mut session = Session::default();
        let blockchain = ModuleId::from("blockchain_module");
        let capability = ModuleId::from("capability_module");

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![
            reported("blockchain_module", "loaded"),
            reported("capability_module", "not_loaded"),
        ]));

        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));
        session.apply(&Update::Phase(Phase::Restarting { attempt: 2 }));
        session.apply(&Update::Phase(Phase::Failed));

        assert_eq!(
            session.module(&blockchain).unwrap().status,
            Status::Crashed,
            "the module the user was watching die reads as merely off",
        );
        assert_eq!(
            session.module(&capability).unwrap().status,
            Status::NotLoaded,
            "a module that was idle before the restart did not die in it",
        );
    }

    /// A clean quit is the other terminal phase and it means the opposite:
    /// every module is stopped on purpose, so nothing here is evidence of
    /// anything and no row may be accused of dying.
    #[test]
    fn quitting_does_not_accuse_the_modules_it_stops() {
        let mut session = Session::default();
        let blockchain = ModuleId::from("blockchain_module");

        session.apply(&Update::Phase(Phase::Online));
        session.apply(&Update::Modules(vec![reported(
            "blockchain_module",
            "loaded",
        )]));

        session.apply(&Update::Phase(Phase::Restarting { attempt: 1 }));
        session.apply(&Update::Phase(Phase::ShuttingDown));

        assert_eq!(
            session.module(&blockchain).unwrap().status,
            Status::NotLoaded,
        );
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
