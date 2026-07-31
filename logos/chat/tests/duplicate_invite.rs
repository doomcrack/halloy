//! Live duplicate-invite probe: B creates a direct conversation with A's
//! address TWICE, and A has to survive both welcomes.
//!
//! This is the end-to-end half of the investigation into a reported
//! `SIGABRT` in `chat_module` on a duplicate MLS Welcome
//! (`GroupV1Convo::new_from_welcome`, whose `StagedWelcome` chain unwrapped
//! before the fix). The report's suggested trigger was "invite the same
//! peer twice", and this test is what settles whether that trigger is real
//! against a live fleet.
//!
//! What a crash would look like from here, and why this test can see it:
//! `chat_module` is built with `panic = "abort"`, so a panic inside the
//! module aborts the whole `logoscore` process. A would then lose its
//! daemon mid-run — the session reports `Fatal`/`Stopped`, the second
//! `conversation_created` never arrives, and the daemon log carries the
//! abort. So the assertions below are the crash detector: A must observe
//! BOTH invites, and its daemon log must contain none of the abort markers
//! in [`CRASH_MARKERS`]. The log is scanned even on the paths that pass,
//! because a module that died and was restarted underneath a session could
//! otherwise go unnoticed.
//!
//! Opt-in exactly like `tests/two_instance.rs`, whose harness this mirrors:
//!
//! ```text
//! LOGOS_LIVE_TESTS=1 LOGOS_LIVE_NETWORK=1 cargo test -p logos-chat \
//!     --features ffi --test duplicate_invite -- --nocapture
//! ```
//!
//! Roles are as in `two_instance`: this process is A, the invited side, and
//! it owns both temp dirs; the peer B is a re-execution of this binary and
//! is the inviter. See that file for why the peer needs its own process.
//!
//! The exchange leg at the end is deliberate. Proving A did not abort is
//! only half of it — a module that survived by wedging its inbox would be
//! just as broken, so B sends on the SECOND conversation and waits for A's
//! reply over it. That both conversations are distinct is asserted too: a
//! stack that silently folded the second invite into the first would be a
//! different bug, not a pass.
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
/// Key-package fetch, the invite's trip over the fleet, and the receiving
/// module's processing of it. Spent per invite, and the second one is the
/// whole point of this test, so it gets the same room as the first.
const INVITE_CAP: Duration = Duration::from_secs(180);
/// One message over the fleet, in either direction.
const MESSAGE_CAP: Duration = Duration::from_secs(180);
/// How long B waits for A's reply before sending its message again.
const SEND_RETRY: Duration = Duration::from_secs(20);
/// The peer's own shutdown, once it has everything it was waiting for.
const PEER_EXIT_CAP: Duration = Duration::from_secs(90);
/// How long a peer that already failed may take to go away by itself.
const PEER_KILL_CAP: Duration = Duration::from_secs(10);
/// One clean shutdown.
const QUIT_CAP: Duration = Duration::from_secs(60);
/// How long a daemon may take to disappear from the process table.
const REAPED_CAP: Duration = Duration::from_secs(15);
/// How long a wait for the peer runs before the session is pumped again.
const PEER_POLL: Duration = Duration::from_millis(250);
/// Lines of a log printed with a failure.
const LOG_TAIL_LINES: usize = 60;

/// What an aborted module leaves in the daemon's log. `signal 6` is the
/// SIGABRT a Rust panic raises under `panic = "abort"`; the others catch
/// the host noticing the module went away, and the panic line itself.
const CRASH_MARKERS: &[&str] = &[
    "signal 6",
    "SIGABRT",
    "panicked at",
    "crashed",
    "not_loaded",
];

/// The peer's state dir. Set only by the parent process.
const PEER_STATE_DIR: &str = "LOGOS_DUP_INVITE_STATE_DIR";
/// A's address, which the peer creates both conversations against.
const PEER_ADDRESS: &str = "LOGOS_DUP_INVITE_ADDRESS";
/// What the peer sends on the second conversation, and waits for back.
const PEER_HELLO: &str = "LOGOS_DUP_INVITE_HELLO";
const PEER_REPLY: &str = "LOGOS_DUP_INVITE_REPLY";
/// The peer's verdict, and its console, both beside its state dir.
const PEER_STATUS_FILE: &str = "peer-status";
const PEER_CONSOLE_FILE: &str = "peer-console.log";
/// The peer ran and did everything asked of it.
const STATUS_EXCHANGED: &str = "EXCHANGED";
/// The peer's delivery node never reached the fleet.
const STATUS_NO_FLEET: &str = "NOFLEET";
/// The test the parent re-executes this binary to run.
const PEER_TEST: &str = "peer_side_invites_twice";

/// Why a run stopped, when it stopped without failing.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Exchanged,
    NoFleet,
}

/// How a wait that also watches the peer process ended.
enum PeerWait {
    Reached,
    Exited(String),
    Expired,
}

/// One live session, plus everything the assertions read back out of its
/// update stream.
struct Live {
    label: &'static str,
    state_dir: PathBuf,
    updates: Pin<Box<dyn Stream<Item = Update>>>,
    started: Instant,
    control: Option<mpsc::Sender<Control>>,
    address: Option<String>,
    online: bool,
    /// EVERY `conversation_created`, in arrival order, as (conversation,
    /// `is_outgoing`). Unlike `two_instance`, which keeps only the first,
    /// this test is about the second one arriving at all.
    invites: Vec<(ConvoId, bool)>,
    /// Every `message_received`, as (conversation, content).
    inbox: Vec<(ConvoId, String)>,
    stopped: bool,
    last_failure: Option<String>,
    fatal: Option<String>,
}

/// Side A: the invited side, invited twice, and the process that drives
/// the run.
#[tokio::test]
async fn a_survives_being_invited_twice_by_the_same_peer() {
    if std::env::var("LOGOS_LIVE_TESTS").as_deref() != Ok("1")
        || std::env::var("LOGOS_LIVE_NETWORK").as_deref() != Ok("1")
    {
        eprintln!(
            "SKIP duplicate-invite probe: it spawns TWO daemons and needs \
             both the fleet and the key-package registry, so it runs only \
             with LOGOS_LIVE_TESTS=1 AND LOGOS_LIVE_NETWORK=1, plus \
             LOGOSCORE_BIN, LOGOS_MODULES_DIR and LOGOS_PROTOCOL_ROOT from \
             `. scripts/dev-env.sh` (or `nix develop`)"
        );
        return;
    }

    let root_a = tempfile::tempdir().expect("temp dir for A");
    let root_b = tempfile::tempdir().expect("temp dir for the peer");
    let mut a = Live::start("A", root_a.path().join("frigicom"));
    let peer_state_dir = root_b.path().join("frigicom");
    eprintln!(
        "dup: logoscore={:?} modules={:?}\ndup: A={}\ndup: B={}",
        env_path("LOGOSCORE_BIN"),
        env_path("LOGOS_MODULES_DIR"),
        a.state_dir.display(),
        peer_state_dir.display()
    );

    let outcome = exchange(&mut a, root_b.path(), &peer_state_dir).await;
    let shutdown = a.shutdown().await;
    let reaped = reaped(&peer_state_dir);
    replay_peer_console(root_b.path());

    // The log scan is the crash detector, and it runs on every path out —
    // including a pass, since a module that aborted and was restarted
    // underneath the session could otherwise be missed.
    let a_crash = crash_report(&a.state_dir, "A");
    let b_crash = crash_report(&peer_state_dir, "the peer");

    let failures: Vec<String> = [
        outcome.as_ref().err().cloned(),
        shutdown.err(),
        reaped.err(),
        a_crash.clone(),
        b_crash.clone(),
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
            "dup: A took two invites from the same peer in {:?}, no module \
             abort in either daemon log, and the second conversation \
             carried traffic both ways",
            a.started.elapsed()
        ),
        Ok(Outcome::NoFleet) => eprintln!(
            "dup: skipped after {:?}, both sessions down clean",
            a.started.elapsed()
        ),
        Err(_) => unreachable!("a failed run already bailed"),
    }
}

/// Side B: the inviter, twice over.
#[tokio::test]
async fn peer_side_invites_twice() {
    let Some(state_dir) = std::env::var_os(PEER_STATE_DIR) else {
        eprintln!(
            "SKIP the peer side: it is the second process of \
             a_survives_being_invited_twice_by_the_same_peer, which \
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

/// A's ladder, the peer's whole run, and the two invites between them.
async fn exchange(
    a: &mut Live,
    root_b: &Path,
    peer_state_dir: &Path,
) -> Result<Outcome, String> {
    if !a.reach_online().await {
        eprintln!(
            "SKIP the probe: A never reported Online (delivery within \
             {ONLINE_CAP:?}) — no fleet reachable from this sandbox."
        );
        return Ok(Outcome::NoFleet);
    }

    let step = Instant::now();
    if !a.pump(ADDRESS_CAP, |live| live.address.is_some()).await {
        return Err(a.why("A's address (the fresh-online Ready)"));
    }
    let address = a.address.clone().expect("just waited for it");
    a.log(&format!("address is {address}"), step);

    let nonce = format!("{}-{}", std::process::id(), file_name(root_b));
    let hello = format!("frigicom duplicate-invite hello {nonce}");
    let reply = format!("frigicom duplicate-invite reply {nonce}");

    let mut peer =
        spawn_peer(root_b, peer_state_dir, &address, &hello, &reply)?;
    let legs = legs(a, &mut peer, &hello, &reply).await;
    settle_peer(&mut peer);

    let status = std::fs::read_to_string(root_b.join(PEER_STATUS_FILE))
        .map_or_else(
            |error| format!("unreadable ({error})"),
            |status| status.trim().to_owned(),
        );
    if status == STATUS_NO_FLEET {
        eprintln!(
            "SKIP the probe: the peer never reported Online — no fleet \
             reachable from this sandbox."
        );
        return Ok(Outcome::NoFleet);
    }
    legs.map_err(|reason| format!("{reason}; the peer reported {status}"))?;
    if status != STATUS_EXCHANGED {
        return Err(format!("the peer reported {status}"));
    }
    Ok(Outcome::Exchanged)
}

/// Everything A does while the peer is running: take one invite, then take
/// a SECOND one from the same peer without dying, then carry traffic.
async fn legs(
    a: &mut Live,
    peer: &mut Child,
    hello: &str,
    reply: &str,
) -> Result<(), String> {
    let step = Instant::now();
    match pump_with_peer(a, peer, INVITE_CAP, |live| !live.invites.is_empty())
        .await
    {
        PeerWait::Reached => {}
        PeerWait::Exited(status) => {
            return Err(format!(
                "the peer exited ({status}) before A saw its first invite"
            ));
        }
        PeerWait::Expired => {
            return Err(a.why("the first conversation_created"));
        }
    }
    let (first, is_outgoing) = a.invites[0].clone();
    if is_outgoing {
        return Err(format!(
            "A was invited to {first} but reported is_outgoing=true"
        ));
    }
    a.log(&format!("took the FIRST invite, {first}"), step);

    // The whole point. Before the fix this is where an unpatched module
    // would be expected to die if a repeated invite really did replay a
    // welcome the receiver had already joined.
    let step = Instant::now();
    match pump_with_peer(a, peer, INVITE_CAP, |live| live.invites.len() >= 2)
        .await
    {
        PeerWait::Reached => {}
        PeerWait::Exited(status) => {
            return Err(format!(
                "the peer exited ({status}) before A saw its SECOND invite"
            ));
        }
        PeerWait::Expired => {
            return Err(format!(
                "{} — A survived long enough to be asked, so if its module \
                 aborted the daemon log says so",
                a.why("the SECOND conversation_created")
            ));
        }
    }
    let (second, is_outgoing) = a.invites[1].clone();
    if is_outgoing {
        return Err(format!(
            "A was invited to {second} but reported is_outgoing=true"
        ));
    }
    // Folding the two together would be a different bug, not a pass.
    if second == first {
        return Err(format!(
            "A reported the second invite on the same conversation {first}; \
             two separate invites must be two separate conversations"
        ));
    }
    a.log(&format!("took the SECOND invite, {second}"), step);

    // Surviving is not enough: the module must still work. B sends on one
    // of the two conversations and A's reply over that same one proves the
    // group is live.
    //
    // Which one it is cannot be assumed. The two `conversation_created`
    // events race each other over the fleet, so A's arrival order need not
    // be B's creation order — in practice it often is not. What matters is
    // that the message lands on a conversation A was actually invited to,
    // so that is what gets asserted, and the reply goes back on whichever
    // one it turned out to be.
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
    let landed = a.landed(hello).expect("just waited for it");
    if landed != first && landed != second {
        return Err(format!(
            "A received {hello:?} on {landed}, which is neither of the two \
             conversations it was invited to ({first}, {second})"
        ));
    }
    a.log(&format!("received {hello:?} on {landed}"), step);

    let step = Instant::now();
    a.send(Control::SendMessage {
        convo_id: landed,
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

/// The peer's own side: reach the fleet, invite A, invite A AGAIN, then
/// send on the second conversation and wait for A's reply.
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
    if !b.pump(INVITE_CAP, |live| !live.invites.is_empty()).await {
        return Err(
            b.why(&format!("the first conversation_created for {address}"))
        );
    }
    let (first, is_outgoing) = b.invites[0].clone();
    if !is_outgoing {
        return Err(format!(
            "B created {first} but reported is_outgoing=false"
        ));
    }
    b.log(&format!("created the FIRST conversation {first}"), step);

    // The second invite to the same address. B waits for its own
    // confirmation of it before sending anything, so a failure here is
    // B's own module and not A's.
    let step = Instant::now();
    b.send(Control::CreateConversation {
        peer_address: address.clone(),
    })?;
    if !b.pump(INVITE_CAP, |live| live.invites.len() >= 2).await {
        return Err(
            b.why(&format!("the SECOND conversation_created for {address}"))
        );
    }
    let (second, is_outgoing) = b.invites[1].clone();
    if !is_outgoing {
        return Err(format!(
            "B created {second} but reported is_outgoing=false"
        ));
    }
    if second == first {
        return Err(format!(
            "B's second create_conversation returned the same conversation \
             {first}; this probe needs two distinct invites"
        ));
    }
    b.log(&format!("created the SECOND conversation {second}"), step);

    // Same resend loop as `two_instance`: the invited side subscribes to
    // the conversation topic only once it has processed the Welcome, and
    // the delivery nodes run with store disabled, so a message published
    // in that window is lost for good. Resend until A answers.
    let step = Instant::now();
    let wanted = reply.clone();
    let deadline = Instant::now() + MESSAGE_CAP;
    let mut sends = 0;
    loop {
        sends += 1;
        b.send(Control::SendMessage {
            convo_id: second.clone(),
            content: hello.clone(),
        })?;
        b.log(
            &format!("sent on the second conversation (send {sends})"),
            step,
        );

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
    let landed = b.landed(&reply).expect("just waited for it");
    if landed != second {
        return Err(format!(
            "B received {reply:?} on {landed}, not on {second}"
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
            Some(format!("frigicom-duplicate-invite-{label}"));
        Self {
            label,
            state_dir,
            updates: Box::pin(run(config, Driver::Live)),
            started: Instant::now(),
            control: None,
            address: None,
            online: false,
            invites: Vec::new(),
            inbox: Vec::new(),
            stopped: false,
            last_failure: None,
            fatal: None,
        }
    }

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

    /// A conversation already recorded is not recorded again, so a
    /// duplicate `ConversationCreated` delivery cannot fake the second
    /// invite this test is waiting for.
    fn record(&mut self, update: &Update) {
        match update {
            Update::Controller(control) => {
                self.control = Some(control.clone());
            }
            Update::Ready { my_address, .. } => {
                self.address = Some(my_address.clone());
            }
            Update::Phase(Phase::Online) => self.online = true,
            Update::Phase(
                Phase::DeliveryError { .. } | Phase::DeliveryStopped,
            ) => self.online = false,
            Update::Event(ChatEvent::ConversationCreated {
                convo_id,
                is_outgoing,
                ..
            }) => {
                if !self.invites.iter().any(|(seen, _)| seen == convo_id) {
                    self.invites.push((convo_id.clone(), *is_outgoing));
                    eprintln!(
                        "{}: conversation_created #{} {convo_id} \
                         (is_outgoing={is_outgoing})",
                        self.label,
                        self.invites.len()
                    );
                }
            }
            Update::Event(ChatEvent::MessageReceived {
                convo_id,
                content,
                ..
            }) => self.inbox.push((convo_id.clone(), content.clone())),
            Update::Stopped => self.stopped = true,
            Update::ActionFailed(error) => {
                eprintln!("{}: ActionFailed: {error}", self.label);
                self.last_failure = Some(error.to_string());
            }
            Update::Fatal(error) => {
                self.fatal = Some(format!("{} gave up: {error}", self.label));
            }
            _ => {}
        }
    }

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
    eprintln!("dup: the peer had to be killed; it was still running");
}

fn replay_peer_console(root_b: &Path) {
    let Ok(console) = std::fs::read_to_string(root_b.join(PEER_CONSOLE_FILE))
    else {
        return;
    };
    for line in console.lines().filter(|line| line.starts_with("peer B:")) {
        eprintln!("dup: {line}");
    }
}

/// The daemon log lines that say a module aborted, if there are any. This
/// is what turns "the test passed" into "the module did not die", and it
/// is reported as a failure in its own right.
fn crash_report(state_dir: &Path, whose: &str) -> Option<String> {
    let path = state_dir.join("logoscore").join("logoscore.log");
    let contents = std::fs::read_to_string(&path).ok()?;
    let hits: Vec<&str> = contents
        .lines()
        .filter(|line| {
            let lowered = line.to_lowercase();
            CRASH_MARKERS
                .iter()
                .any(|marker| lowered.contains(&marker.to_lowercase()))
        })
        .collect();
    if hits.is_empty() {
        eprintln!("dup: no abort marker in {whose}'s daemon log");
        return None;
    }
    Some(format!(
        "{whose}'s daemon log carries a module abort ({}):\n{}",
        path.display(),
        hits.join("\n")
    ))
}

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
