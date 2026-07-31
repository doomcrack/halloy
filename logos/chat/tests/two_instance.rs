//! Live two-instance exchange: two independent sessions — two daemons, two
//! delivery nodes, two state dirs — meeting on the logos.test fleet. One
//! creates a direct conversation with the other's address, and a message
//! goes each way over it.
//!
//! Opt-in twice over. `LOGOS_LIVE_TESTS=1` (as for `tests/live.rs`) AND
//! `LOGOS_LIVE_NETWORK=1`, because unlike the single-instance ladder this
//! run cannot pass without the network: the invite needs the fleet AND the
//! key-package registry (devnet.chat-kc.logos.co), which is a second thing
//! to be unreachable. Without both it prints a note and returns.
//!
//! ```text
//! LOGOS_LIVE_TESTS=1 LOGOS_LIVE_NETWORK=1 cargo test -p logos-chat \
//!     --features ffi --test two_instance -- --nocapture
//! ```
//!
//! Artifacts come from `LOGOSCORE_BIN` / `LOGOS_MODULES_DIR` and the dylib
//! from `LOGOS_PROTOCOL_ROOT`, exactly as in `tests/live.rs`; use the same
//! staged module tree (`scripts/dev-env.sh` picks it).
//!
//! # Two processes, not two sessions
//!
//! The second instance runs in a re-execution of THIS test binary, driven
//! by `peer_side_creates_the_conversation_and_replies`, and not as a second
//! session beside the first. It has to: the protocol library stores auth
//! tokens in a process-wide singleton keyed by TARGET MODULE name and looks
//! the token up on every call, so a second `lp_client` for `core_service`
//! overwrites the first one's token. Both sessions then present one
//! daemon's token to both daemons, and the loser's calls come back as
//! `null` — which the gateway reads as an unsupported method or a dead
//! link. One live backend per process is a property of the library, so the
//! peer gets its own.
//!
//! Roles: this process is A, the invited side, and it stays in charge —
//! it owns both temp dirs, hands the peer its state dir and A's address,
//! and waits for it to exit. The peer is B, the inviter.
//!
//! What is asserted once both sides are Online: A's `get_address`, then B's
//! `create_direct_conversation` against it landing as
//! `ConversationCreated { is_outgoing: true }` on B and, over the fleet, as
//! `ConversationCreated { is_outgoing: false }` on A; then a message B → A
//! and a reply A → B, each arriving as `MessageReceived` on the receiver's
//! own conversation with the content that was sent. The contents carry a
//! per-run nonce, so a stale message from an earlier run cannot satisfy any
//! of it, and every wait is a condition over recorded state rather than a
//! match on the update in hand — a duplicate delivery changes nothing.
//!
//! A side that never reaches Online means no fleet from this sandbox: that
//! prints a skip note and passes, like the online leg of `tests/live.rs`.
//! Everything past that point is a real failure and is reported as one,
//! with ONE exception, which is a property of the stack under test rather
//! than of this run: the invited side subscribes to the conversation's
//! content topic only when it processes the Welcome, and the delivery
//! nodes run with store disabled, so a message published in that window is
//! dropped and cannot be fetched later. B's first send lands in it — the
//! daemon log shows A's node taking the message off relay ~50ms before its
//! own `subscribe` for that topic — so B resends until A answers and says
//! in its console how many sends went nowhere. Reading a lost first
//! message as a test failure would only hide it.
//!
//! Both sessions are always quit down to `Update::Stopped` and checked for
//! a logoscore that still holds their state dir — including on the skip
//! path and after a failed step, which is why the steps report an error
//! upward instead of panicking where they stand. The panic that ends a
//! failed run carries the tail of BOTH daemon logs and of the peer's
//! console, since the temp dirs go away as it unwinds.
#![cfg(feature = "ffi")]

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use futures::stream::{Stream, StreamExt};
use logos_chat::session::run;
use logos_chat::{
    BackendConfig, ChatEvent, Control, ConvoId, Driver, Phase, Update,
};
use logos_daemon::ArtifactOverrides;

/// Daemon spawn + loadModule + init, before delivery gets involved.
const LADDER_CAP: Duration = Duration::from_secs(120);
/// Delivery's window to reach the fleet, spent by each side separately.
const ONLINE_CAP: Duration = Duration::from_secs(90);
/// The fresh-online resync that carries `get_address`.
const ADDRESS_CAP: Duration = Duration::from_secs(60);
/// Key-package fetch from the registry, the invite's trip over the fleet,
/// and the receiving module's processing of it.
const INVITE_CAP: Duration = Duration::from_secs(180);
/// One message over the fleet, in either direction.
const MESSAGE_CAP: Duration = Duration::from_secs(180);
/// How long B waits for A's reply before sending its message again; see the
/// resend loop in [`peer_exchange`] for why one send is not enough.
const SEND_RETRY: Duration = Duration::from_secs(20);
/// The peer's own shutdown, once it has everything it was waiting for.
const PEER_EXIT_CAP: Duration = Duration::from_secs(90);
/// How long a peer that already failed may take to go away by itself.
const PEER_KILL_CAP: Duration = Duration::from_secs(10);
/// One clean shutdown.
const QUIT_CAP: Duration = Duration::from_secs(60);
/// How long a daemon may take to disappear from the process table.
const REAPED_CAP: Duration = Duration::from_secs(15);
/// How long a wait for the peer runs before the session is pumped again;
/// A makes no progress while nobody polls its stream.
const PEER_POLL: Duration = Duration::from_millis(250);
/// Lines of a log printed with a failure.
const LOG_TAIL_LINES: usize = 40;

/// The peer's state dir. Set only by the parent process — its presence is
/// what turns the peer test from a skip into a run.
const PEER_STATE_DIR: &str = "LOGOS_TWO_INSTANCE_STATE_DIR";
/// A's address, which the peer creates the conversation against.
const PEER_ADDRESS: &str = "LOGOS_TWO_INSTANCE_ADDRESS";
/// What the peer sends, and what it then waits for in return.
const PEER_HELLO: &str = "LOGOS_TWO_INSTANCE_HELLO";
const PEER_REPLY: &str = "LOGOS_TWO_INSTANCE_REPLY";
/// The peer's verdict, and its console, both beside its state dir.
const PEER_STATUS_FILE: &str = "peer-status";
const PEER_CONSOLE_FILE: &str = "peer-console.log";
/// The peer ran and did everything asked of it.
const STATUS_EXCHANGED: &str = "EXCHANGED";
/// The peer's delivery node never reached the fleet.
const STATUS_NO_FLEET: &str = "NOFLEET";
/// The test the parent re-executes this binary to run.
const PEER_TEST: &str = "peer_side_creates_the_conversation_and_replies";

/// Why a run stopped, when it stopped without failing.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Exchanged,
    NoFleet,
}

/// How a wait that also watches the peer process ended.
enum PeerWait {
    /// The condition held.
    Reached,
    /// The peer exited first, with this status rendered.
    Exited(String),
    /// The window closed with the peer still running.
    Expired,
}

/// One live session, plus everything the assertions read back out of its
/// update stream.
struct Live {
    /// Names this side in every line it prints.
    label: &'static str,
    state_dir: PathBuf,
    updates: Pin<Box<dyn Stream<Item = Update>>>,
    started: Instant,
    control: Option<mpsc::Sender<Control>>,
    address: Option<String>,
    online: bool,
    online_at: Option<Duration>,
    /// First `conversation_created`, as (conversation, `is_outgoing`).
    invite: Option<(ConvoId, bool)>,
    /// Every `message_received`, as (conversation, content).
    inbox: Vec<(ConvoId, String)>,
    stopped: bool,
    last_failure: Option<String>,
    /// Set by a `Fatal` (or an ended stream) — nothing more will arrive.
    fatal: Option<String>,
}

/// Side A: the invited side, and the process that drives the run.
#[tokio::test]
async fn two_instances_exchange_a_direct_conversation() {
    if std::env::var("LOGOS_LIVE_TESTS").as_deref() != Ok("1")
        || std::env::var("LOGOS_LIVE_NETWORK").as_deref() != Ok("1")
    {
        eprintln!(
            "SKIP two-instance exchange: it spawns TWO daemons and needs \
             both the logos.test fleet and the key-package registry, so it \
             runs only with LOGOS_LIVE_TESTS=1 AND LOGOS_LIVE_NETWORK=1, \
             plus LOGOSCORE_BIN, LOGOS_MODULES_DIR and LOGOS_PROTOCOL_ROOT \
             from `. scripts/dev-env.sh` (or `nix develop`)"
        );
        return;
    }

    // Both temp dirs belong to this process: the peer only borrows its
    // one, so its daemon log is still here to be read after it exits.
    let root_a = tempfile::tempdir().expect("temp dir for A");
    let root_b = tempfile::tempdir().expect("temp dir for the peer");
    let mut a = Live::start("A", root_a.path().join("frigicom"));
    let peer_state_dir = root_b.path().join("frigicom");
    eprintln!(
        "two: logoscore={:?} modules={:?}\ntwo: A={}\ntwo: B={}",
        env_path("LOGOSCORE_BIN"),
        env_path("LOGOS_MODULES_DIR"),
        a.state_dir.display(),
        peer_state_dir.display()
    );

    // Reported upward rather than panicked on, so the shutdown assertions
    // still run over a failed exchange and A's daemon still goes down
    // clean.
    let outcome = exchange(&mut a, root_b.path(), &peer_state_dir).await;
    let shutdown = a.shutdown().await;
    let reaped = reaped(&peer_state_dir);
    replay_peer_console(root_b.path());

    let failures: Vec<String> = [
        outcome.as_ref().err().cloned(),
        shutdown.err(),
        reaped.err(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !failures.is_empty() {
        bail(
            &failures.join("\n"),
            &[&a.state_dir, &peer_state_dir],
            root_b.path(),
        );
    }
    match outcome {
        Ok(Outcome::Exchanged) => eprintln!(
            "two: full exchange in {:?}, both sessions down clean",
            a.started.elapsed()
        ),
        Ok(Outcome::NoFleet) => eprintln!(
            "two: skipped the exchange after {:?}, both sessions down clean",
            a.started.elapsed()
        ),
        Err(_) => unreachable!("a failed exchange already bailed"),
    }
}

/// Side B: the inviter. A skip in its own right — the parent process is
/// what sets [`PEER_STATE_DIR`], and nothing else should run this.
#[tokio::test]
async fn peer_side_creates_the_conversation_and_replies() {
    let Some(state_dir) = std::env::var_os(PEER_STATE_DIR) else {
        eprintln!(
            "SKIP the peer side: it is the second process of \
             two_instances_exchange_a_direct_conversation, which \
             re-executes this binary with {PEER_STATE_DIR} set"
        );
        return;
    };

    let state_dir = PathBuf::from(state_dir);
    let root = state_dir
        .parent()
        .expect("the peer state dir always has a parent")
        .to_owned();
    let mut b = Live::start("peer B", state_dir);

    let outcome = peer_exchange(&mut b).await;
    let shutdown = b.shutdown().await;

    let verdict = match (&outcome, shutdown) {
        (Ok(outcome), Ok(())) => match outcome {
            Outcome::Exchanged => STATUS_EXCHANGED.to_owned(),
            Outcome::NoFleet => STATUS_NO_FLEET.to_owned(),
        },
        (Err(reason), Ok(())) => format!("FAILED: {reason}"),
        (Ok(_), Err(reason)) => format!("FAILED: {reason}"),
        (Err(reason), Err(other)) => format!("FAILED: {reason}; {other}"),
    };
    eprintln!("peer B: verdict {verdict}");
    std::fs::write(root.join(PEER_STATUS_FILE), &verdict)
        .expect("the peer must be able to report its verdict");

    assert!(
        verdict == STATUS_EXCHANGED || verdict == STATUS_NO_FLEET,
        "{verdict}\n{}",
        log_tail(&b.state_dir, "peer B's daemon")
    );
}

/// A's ladder, the peer's whole run, and the two messages between them.
/// `Err` describes the first step that did not happen; the caller shuts
/// down before reporting it.
async fn exchange(
    a: &mut Live,
    root_b: &Path,
    peer_state_dir: &Path,
) -> Result<Outcome, String> {
    if !a.reach_online().await {
        eprintln!(
            "SKIP the exchange: A never reported Online (delivery within \
             {ONLINE_CAP:?}) — no logos.test fleet reachable from this \
             sandbox. Shutdown assertions still run."
        );
        return Ok(Outcome::NoFleet);
    }

    let step = Instant::now();
    if !a.pump(ADDRESS_CAP, |live| live.address.is_some()).await {
        return Err(a.why("A's address (the fresh-online Ready)"));
    }
    let address = a.address.clone().expect("just waited for it");
    a.log(&format!("address is {address}"), step);

    // The nonce keeps this run's traffic apart from any earlier run's: the
    // temp dir name is fresh per run and the pid per process, which is
    // enough without a clock.
    let nonce = format!("{}-{}", std::process::id(), file_name(root_b));
    let hello = format!("frigicom two-instance hello {nonce}");
    let reply = format!("frigicom two-instance reply {nonce}");

    let mut peer =
        spawn_peer(root_b, peer_state_dir, &address, &hello, &reply)?;
    let legs = legs(a, &mut peer, &hello, &reply).await;
    settle_peer(&mut peer);

    // A peer that found no fleet makes the whole run a skip, whatever the
    // leg that was waiting on it reported.
    let status = std::fs::read_to_string(root_b.join(PEER_STATUS_FILE))
        .map_or_else(
            |error| format!("unreadable ({error})"),
            |status| status.trim().to_owned(),
        );
    if status == STATUS_NO_FLEET {
        eprintln!(
            "SKIP the exchange: the peer never reported Online — no \
             logos.test fleet reachable from this sandbox. Shutdown \
             assertions still run."
        );
        return Ok(Outcome::NoFleet);
    }
    legs.map_err(|reason| format!("{reason}; the peer reported {status}"))?;
    if status != STATUS_EXCHANGED {
        return Err(format!("the peer reported {status}"));
    }
    Ok(Outcome::Exchanged)
}

/// Everything A does while the peer is running.
async fn legs(
    a: &mut Live,
    peer: &mut Child,
    hello: &str,
    reply: &str,
) -> Result<(), String> {
    let step = Instant::now();
    match pump_with_peer(a, peer, INVITE_CAP, |live| live.invite.is_some())
        .await
    {
        PeerWait::Reached => {}
        PeerWait::Exited(status) => {
            return Err(format!(
                "the peer exited ({status}) before A saw its invite"
            ));
        }
        PeerWait::Expired => {
            return Err(a.why("conversation_created (the invite B sent)"));
        }
    }
    let (convo_id, is_outgoing) = a.invite.clone().expect("just waited");
    if is_outgoing {
        return Err(format!(
            "A was invited to {convo_id} but reported is_outgoing=true"
        ));
    }
    a.log(&format!("was invited to {convo_id}"), step);

    let step = Instant::now();
    let wanted = hello.to_owned();
    match pump_with_peer(a, peer, MESSAGE_CAP, |live| {
        live.inbox.iter().any(|(_, seen)| *seen == wanted)
    })
    .await
    {
        PeerWait::Reached => {}
        PeerWait::Exited(status) => {
            return Err(format!(
                "the peer exited ({status}) before its message reached A"
            ));
        }
        PeerWait::Expired => return Err(a.why(&format!("{hello:?}"))),
    }
    // A message the receiver cannot place on a conversation it has is one
    // its UI could not show.
    let landed = a.landed(hello).expect("just waited for it");
    if landed != convo_id {
        return Err(format!(
            "A received {hello:?} on {landed}, not on its own conversation \
             {convo_id}"
        ));
    }
    a.log(&format!("received {hello:?}"), step);

    // The reply proves the conversation carries traffic both ways, not
    // just from the side that created it. The peer is waiting for exactly
    // this, so its exit is the acknowledgement.
    let step = Instant::now();
    a.send(Control::SendMessage {
        convo_id,
        content: reply.to_owned(),
    })?;
    match pump_with_peer(a, peer, PEER_EXIT_CAP, |_| false).await {
        PeerWait::Exited(status) => {
            a.log(&format!("the peer finished ({status})"), step);
            Ok(())
        }
        _ => Err(format!(
            "the peer never finished after A replied with {reply:?}"
        )),
    }
}

/// The peer process's own side of the exchange: reach the fleet, invite A,
/// send, and wait for A's reply.
async fn peer_exchange(b: &mut Live) -> Result<Outcome, String> {
    let address = env_string(PEER_ADDRESS);
    let hello = env_string(PEER_HELLO);
    let reply = env_string(PEER_REPLY);
    if address.is_empty() || hello.is_empty() || reply.is_empty() {
        return Err(format!(
            "the peer needs {PEER_ADDRESS}, {PEER_HELLO} and {PEER_REPLY}"
        ));
    }

    if !b.reach_online().await {
        eprintln!("peer B: never reported Online within {ONLINE_CAP:?}");
        return Ok(Outcome::NoFleet);
    }

    let step = Instant::now();
    b.send(Control::CreateConversation {
        peer_address: address.clone(),
    })?;
    if !b.pump(INVITE_CAP, |live| live.invite.is_some()).await {
        return Err(b.why(&format!("conversation_created for {address}")));
    }
    let (convo_id, is_outgoing) = b.invite.clone().expect("just waited");
    if !is_outgoing {
        return Err(format!(
            "B created {convo_id} but reported is_outgoing=false"
        ));
    }
    b.log(&format!("created {convo_id} with A"), step);

    // A's reply is the only acknowledgement there is, and the first send
    // usually does not earn one: the invitee subscribes to the
    // conversation's content topic only once it has processed the Welcome,
    // and the delivery nodes run with store disabled, so a message
    // published in that window is dropped and never recoverable. Resend
    // until A answers rather than fail over a race this test does not own;
    // the attempt count is what says the race was hit.
    let step = Instant::now();
    let wanted = reply.clone();
    let deadline = Instant::now() + MESSAGE_CAP;
    let mut sends = 0;
    loop {
        sends += 1;
        b.send(Control::SendMessage {
            convo_id: convo_id.clone(),
            content: hello.clone(),
        })?;
        b.log(&format!("sent its message to A (send {sends})"), step);

        let left = deadline.saturating_duration_since(Instant::now());
        if b.pump(SEND_RETRY.min(left), |live| {
            live.inbox.iter().any(|(_, seen)| *seen == wanted)
        })
        .await
        {
            break;
        }
        if b.fatal.is_some() || Instant::now() >= deadline {
            return Err(b.why(&format!("{reply:?} after {sends} send(s)")));
        }
    }
    if sends > 1 {
        eprintln!(
            "peer B: NOTE {} send(s) went nowhere before A answered — a \
             message published before the invited side subscribes to the \
             conversation topic is lost for good",
            sends - 1
        );
    }
    let landed = b.landed(&reply).expect("just waited for it");
    if landed != convo_id {
        return Err(format!(
            "B received {reply:?} on {landed}, not on its own conversation \
             {convo_id}"
        ));
    }
    b.log(&format!("received {reply:?} after {sends} send(s)"), step);

    Ok(Outcome::Exchanged)
}

impl Live {
    fn start(label: &'static str, state_dir: PathBuf) -> Self {
        let mut config = BackendConfig::new(state_dir.clone());
        config.artifacts = ArtifactOverrides {
            logoscore_bin: env_path("LOGOSCORE_BIN"),
            modules_dir: env_path("LOGOS_MODULES_DIR"),
        };
        config.installation_name =
            Some(format!("frigicom-two-instance-{label}"));
        Self {
            label,
            state_dir,
            updates: Box::pin(run(config, Driver::Live)),
            started: Instant::now(),
            control: None,
            address: None,
            online: false,
            online_at: None,
            invite: None,
            inbox: Vec::new(),
            stopped: false,
            last_failure: None,
            fatal: None,
        }
    }

    /// The controller, then the whole ladder up to a delivery node that
    /// reached the fleet.
    async fn reach_online(&mut self) -> bool {
        let step = Instant::now();
        if !self.pump(LADDER_CAP, |live| live.control.is_some()).await {
            return false;
        }
        if !self.pump(ONLINE_CAP, |live| live.online).await {
            return false;
        }
        self.log("delivery Online", step);
        true
    }

    /// Pumps the session until `done` holds — checked before the first
    /// await, so a condition already satisfied does not wait for an update
    /// that may never come. `false` means the window closed first (or the
    /// session gave up); the caller decides what that means.
    async fn pump(
        &mut self,
        within: Duration,
        done: impl Fn(&Self) -> bool,
    ) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if done(self) {
                return true;
            }
            if self.fatal.is_some() {
                return false;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            match tokio::time::timeout(left, self.updates.next()).await {
                Ok(Some(update)) => self.record(&update),
                Ok(None) => {
                    self.fatal = Some("the update stream ended".to_owned());
                }
                Err(_) => return false,
            }
        }
    }

    /// Everything the assertions read later. A repeat of an event already
    /// recorded leaves the recorded value alone, so a duplicate delivery
    /// cannot turn a satisfied condition back off.
    fn record(&mut self, update: &Update) {
        let elapsed = self.started.elapsed();
        match update {
            Update::Controller(control) => {
                self.control = Some(control.clone());
            }
            Update::Ready { my_address, .. } => {
                self.address = Some(my_address.clone());
            }
            Update::Phase(Phase::Online) => {
                self.online = true;
                self.online_at.get_or_insert(elapsed);
            }
            Update::Phase(
                Phase::DeliveryError { .. } | Phase::DeliveryStopped,
            ) => self.online = false,
            Update::Event(ChatEvent::ConversationCreated {
                convo_id,
                is_outgoing,
                ..
            }) => {
                self.invite
                    .get_or_insert_with(|| (convo_id.clone(), *is_outgoing));
            }
            Update::Event(ChatEvent::MessageReceived {
                convo_id,
                content,
                ..
            }) => self.inbox.push((convo_id.clone(), content.clone())),
            Update::Stopped => self.stopped = true,
            Update::ActionFailed(error) => {
                // Never fatal on its own — an automatic member reload can
                // fail without the exchange being in trouble — but it is
                // the first thing to read when a step then times out.
                eprintln!("{}: ActionFailed: {error}", self.label);
                self.last_failure = Some(error.to_string());
            }
            Update::Fatal(error) => {
                self.fatal = Some(format!("{} gave up: {error}", self.label));
            }
            _ => {}
        }
    }

    /// The conversation `content` arrived on, if it arrived at all.
    fn landed(&self, content: &str) -> Option<ConvoId> {
        self.inbox
            .iter()
            .find(|(_, seen)| seen == content)
            .map(|(convo_id, _)| convo_id.clone())
    }

    fn send(&mut self, control: Control) -> Result<(), String> {
        let described = format!("{control:?}");
        let sent = match self.control.as_mut() {
            Some(sender) => sender.try_send(control).map_err(|e| e.to_string()),
            None => Err("it never handed out a controller".to_owned()),
        };
        sent.map_err(|reason| {
            format!("{} could not send {described}: {reason}", self.label)
        })
    }

    /// Quits the session and holds it to leaving nothing behind. Runs on
    /// every path out of a role, so it reports rather than panics.
    async fn shutdown(&mut self) -> Result<(), String> {
        if let Some(fatal) = &self.fatal {
            return Err(format!("{fatal} — nothing left to quit"));
        }
        let step = Instant::now();
        self.send(Control::Quit)?;
        if !self.pump(QUIT_CAP, |live| live.stopped).await {
            return Err(self.why("Update::Stopped"));
        }
        self.log("Stopped", step);
        reaped(&self.state_dir)
    }

    /// Why a wait ended without its condition holding — the session that
    /// gave up, or whatever it last reported failing.
    fn why(&self, waited_for: &str) -> String {
        let mut reason = format!("{} never saw {waited_for}", self.label);
        if let Some(fatal) = &self.fatal {
            reason.push_str(&format!("; {fatal}"));
        }
        if let Some(failure) = &self.last_failure {
            reason.push_str(&format!("; last failure: {failure}"));
        }
        reason
    }

    fn log(&self, what: &str, step: Instant) {
        eprintln!(
            "{}: {what} — {:?} for this step, {:?} into the run",
            self.label,
            step.elapsed(),
            self.started.elapsed()
        );
    }
}

/// Pumps A until `done` holds, the peer exits, or the window closes. A
/// makes no progress while its stream is unpolled, so the peer can only be
/// checked between pumps.
async fn pump_with_peer(
    a: &mut Live,
    peer: &mut Child,
    within: Duration,
    done: impl Fn(&Live) -> bool,
) -> PeerWait {
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if a.pump(PEER_POLL.min(left), &done).await {
            return PeerWait::Reached;
        }
        match peer.try_wait() {
            Ok(Some(status)) => return PeerWait::Exited(status.to_string()),
            Ok(None) => {}
            Err(error) => {
                return PeerWait::Exited(format!("unwaitable: {error}"));
            }
        }
        if left.is_zero() {
            return PeerWait::Expired;
        }
    }
}

/// Re-executes this test binary as the peer, with its console redirected
/// into the temp dir the parent will read it from.
fn spawn_peer(
    root_b: &Path,
    state_dir: &Path,
    address: &str,
    hello: &str,
    reply: &str,
) -> Result<Child, String> {
    let binary = std::env::current_exe()
        .map_err(|error| format!("no path to this test binary: {error}"))?;
    let console = std::fs::File::create(root_b.join(PEER_CONSOLE_FILE))
        .map_err(|error| format!("no console for the peer: {error}"))?;
    let errors = console
        .try_clone()
        .map_err(|error| format!("no console for the peer: {error}"))?;

    Command::new(&binary)
        .args(["--exact", PEER_TEST, "--nocapture"])
        .env(PEER_STATE_DIR, state_dir)
        .env(PEER_ADDRESS, address)
        .env(PEER_HELLO, hello)
        .env(PEER_REPLY, reply)
        .stdin(Stdio::null())
        .stdout(Stdio::from(console))
        .stderr(Stdio::from(errors))
        .spawn()
        .map_err(|error| {
            format!("could not run {} as the peer: {error}", binary.display())
        })
}

/// Makes sure the peer is gone before the run is judged: one that is still
/// waiting on something it will never get is killed rather than left to
/// outlive this process.
fn settle_peer(peer: &mut Child) {
    let deadline = Instant::now() + PEER_KILL_CAP;
    while Instant::now() < deadline {
        match peer.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(PEER_POLL),
            Err(_) => break,
        }
    }
    let _ = peer.kill();
    let _ = peer.wait();
    eprintln!("two: the peer had to be killed; it was still running");
}

/// Prints what the peer reported, so one console carries both sides'
/// timings.
fn replay_peer_console(root_b: &Path) {
    let Ok(console) = std::fs::read_to_string(root_b.join(PEER_CONSOLE_FILE))
    else {
        return;
    };
    for line in console.lines().filter(|line| line.starts_with("peer B:")) {
        eprintln!("two: {line}");
    }
}

/// Holds a state dir to having no logoscore left on it.
fn reaped(state_dir: &Path) -> Result<(), String> {
    let deadline = Instant::now() + REAPED_CAP;
    let mut stray = stray_processes(state_dir);
    while !stray.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        stray = stray_processes(state_dir);
    }
    if stray.is_empty() {
        return Ok(());
    }
    Err(format!(
        "logoscore still holds {}:\n{stray}",
        state_dir.display()
    ))
}

/// Processes whose command line still mentions a state dir.
fn stray_processes(state_dir: &Path) -> String {
    let Ok(output) = Command::new("pgrep")
        .arg("-fl")
        .arg(state_dir.to_string_lossy().as_ref())
        .output()
    else {
        return String::new();
    };
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from)
}

fn env_string(key: &str) -> String {
    std::env::var(key).unwrap_or_default()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The tail of a daemon's own account of a run.
fn log_tail(state_dir: &Path, whose: &str) -> String {
    let path = state_dir.join("logoscore").join("logoscore.log");
    format!(
        "--- tail of {} ({whose}) ---\n{}",
        path.display(),
        tail(&path)
    )
}

fn tail(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Err(error) => format!("({error})"),
        Ok(contents) => {
            let lines: Vec<&str> = contents.lines().collect();
            lines[lines.len().saturating_sub(LOG_TAIL_LINES)..].join("\n")
        }
    }
}

/// Fails with both daemons' logs and the peer's console attached: the temp
/// dirs go away as the panic unwinds, so this is the only chance to read
/// them.
fn bail(message: &str, state_dirs: &[&Path; 2], root_b: &Path) -> ! {
    let console = root_b.join(PEER_CONSOLE_FILE);
    panic!(
        "{message}\n{}\n{}\n--- tail of {} ---\n{}",
        log_tail(state_dirs[0], "A"),
        log_tail(state_dirs[1], "the peer"),
        console.display(),
        tail(&console)
    );
}
