//! Live integration test for the session ladder against a REAL logoscore
//! daemon and the real chat_module. Opt-in — it spawns processes, binds
//! loopback sockets, and (best effort) talks to the logos.test Waku fleet.
//!
//! Run it from the dev shell (`. scripts/dev-env.sh`, or `nix develop`):
//!
//! ```text
//! LOGOS_LIVE_TESTS=1 cargo test -p logos-chat --features ffi --test live \
//!     -- --nocapture
//! ```
//!
//! Artifacts come from `LOGOSCORE_BIN` / `LOGOS_MODULES_DIR` (read here
//! rather than left to `logos_daemon::locate`'s own env lookup so a failure
//! names the exact paths this run used) and the dylib from
//! `LOGOS_PROTOCOL_ROOT`, which `build.rs` turns into an rpath.
//!
//! Asserted unconditionally: `Update::Controller` first; the phase ladder
//! up to `InitialisingChat`; the initial `list_conversations` snapshot and
//! the seeded `status()` phase behind it (both prove `chat.init` returned);
//! then a clean `Control::Quit` that ends in `Update::Stopped`, leaves no
//! logoscore process holding our config dir, and joins the logos thread
//! well inside its detach deadline.
//!
//! The delivery leg (`Phase::Online` → `create_group_conversation` →
//! `conversation_created` → `list_group_members`) waits on the delivery
//! module. It prints a skip note and is passed over when Online never
//! arrives, so an offline sandbox never fails the run.
//!
//! Online, when it does arrive, is held to meaning a delivery node that was
//! created and started: the staged chat_module reads the failure envelope a
//! rejected `createNode`/`start` replies with, so it can no longer report
//! readiness over a node that never came up. The daemon's own log is the
//! witness — Online plus a start-up fault in it fails the run rather than
//! being noted, which is what catches a module tree that quietly went back
//! to claiming readiness (check `LOGOS_MODULES_DIR` first when it fires).
//!
//! Every failure path prints the tail of `<state_dir>/logoscore/
//! logoscore.log` — the daemon's own account of what went wrong — before
//! panicking, because the temp dir is deleted as the test unwinds.
#![cfg(feature = "ffi")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use futures::stream::{Stream, StreamExt};
use logos_chat::session::run;
use logos_chat::{BackendConfig, ChatEvent, Control, Driver, Phase, Update};
use logos_daemon::ArtifactOverrides;

/// Daemon spawn (≤15s) + loadModule (≤30s) + init (≤20s), plus slack for a
/// cold module host.
const LADDER_CAP: Duration = Duration::from_secs(120);
/// Best-effort window for the delivery module to reach the fleet.
const ONLINE_CAP: Duration = Duration::from_secs(90);
/// One module roundtrip (≤20s) and the push events it triggers.
const ACTION_CAP: Duration = Duration::from_secs(60);
/// chat shutdown (≤5s) + daemon shutdown (≤3s) + supervisor grace (≤3s) +
/// the logos-thread join.
const QUIT_CAP: Duration = Duration::from_secs(60);
/// A join that runs into `thread::JOIN_DEADLINE` (25s) detaches the logos
/// thread instead of winding it down; staying under this proves it exited.
const QUIT_BUDGET: Duration = Duration::from_secs(20);
/// How long the daemon may take to disappear from the process table.
const REAPED_CAP: Duration = Duration::from_secs(15);
/// Lines of daemon log printed with a failure.
const LOG_TAIL_LINES: usize = 40;

#[tokio::test]
async fn live_session_ladder_survives_a_real_daemon_and_quits_clean() {
    if std::env::var("LOGOS_LIVE_TESTS").as_deref() != Ok("1") {
        eprintln!(
            "SKIP live session test: set LOGOS_LIVE_TESTS=1 to run it. It \
             also needs LOGOSCORE_BIN, LOGOS_MODULES_DIR and \
             LOGOS_PROTOCOL_ROOT — `. scripts/dev-env.sh` (or `nix develop`) \
             exports all three"
        );
        return;
    }

    let root = tempfile::tempdir().expect("temp dir for the session state");
    let state_dir = root.path().join("frigicom");
    let mut config = BackendConfig::new(state_dir.clone());
    config.artifacts = ArtifactOverrides {
        logoscore_bin: env_path("LOGOSCORE_BIN"),
        modules_dir: env_path("LOGOS_MODULES_DIR"),
    };
    config.installation_name = Some("frigicom-live-test".to_owned());
    eprintln!(
        "live: logoscore={:?} modules={:?} state_dir={}",
        config.artifacts.logoscore_bin,
        config.artifacts.modules_dir,
        state_dir.display()
    );

    let started = Instant::now();
    let mut stream = Box::pin(run(config, Driver::Live));

    let mut control = match first(&mut stream, &state_dir).await {
        Update::Controller(sender) => sender,
        other => bail(
            &state_dir,
            &format!("first update was {other:?}, not Controller"),
        ),
    };

    // The snapshot is the last rung the daemon alone can reach: it lands
    // only if spawn, loadModule, all seven watches and chat.init succeeded.
    let ladder = collect_until(
        &mut stream,
        &state_dir,
        LADDER_CAP,
        "the initial ConversationsSnapshot",
        |update| matches!(update, Update::ConversationsSnapshot(_)),
    )
    .await;
    let ladder_at = started.elapsed();

    let climbed = phases(&ladder);
    let expected = [
        Phase::StartingDaemon,
        Phase::Connecting,
        Phase::LoadingModule,
        Phase::InitialisingChat,
    ];
    if climbed != expected {
        bail(
            &state_dir,
            &format!("phase ladder was {climbed:?}, expected {expected:?}"),
        );
    }
    let Some(Update::ConversationsSnapshot(conversations)) = ladder.last()
    else {
        bail(&state_dir, "the collected ladder did not end in a snapshot");
    };
    eprintln!(
        "live: ladder reached InitialisingChat in {ladder_at:?} with {} \
         conversation(s)",
        conversations.len()
    );

    // The seeded status() roundtrip: the same handler as the live event, so
    // it always emits a phase for whatever the module reports.
    let seeded =
        match next(&mut stream, &state_dir, ACTION_CAP, "status()").await {
            Update::Phase(phase) => phase,
            other => bail(
                &state_dir,
                &format!("expected the seeded status() phase, got {other:?}"),
            ),
        };
    eprintln!(
        "live: status() seeded {seeded:?} at {:?}",
        started.elapsed()
    );

    let online = if seeded == Phase::Online {
        online_leg(&mut stream, &mut control, &state_dir, started).await;
        true
    } else if let Some(seen) =
        pump_until(&mut stream, &state_dir, ONLINE_CAP, |update| {
            matches!(update, Update::Phase(Phase::Online))
        })
        .await
    {
        eprintln!(
            "live: delivery went Online after {:?} ({} update(s) on the way)",
            started.elapsed(),
            seen.len()
        );
        online_leg(&mut stream, &mut control, &state_dir, started).await;
        true
    } else {
        eprintln!(
            "SKIP online leg: delivery never reported Online within \
             {ONLINE_CAP:?} — no logos.test fleet reachable from this \
             sandbox. Shutdown assertions still run."
        );
        false
    };

    // Online is chat_module's verdict, and it is only worth anything because
    // the module now refuses to advance past a `createNode`/`start` that
    // delivery rejected. Hold it to that: a run that reported Online while
    // the daemon logged a delivery start-up fault got its readiness from a
    // node that never came up, and passing it would hide exactly the
    // regression this run exists to catch.
    let faults = delivery_faults(&state_dir);
    if online && !faults.is_empty() {
        bail(
            &state_dir,
            &format!(
                "delivery reported Online over a node that never started: \
                 {}. Is LOGOS_MODULES_DIR the staged tree?",
                faults.join("; ")
            ),
        );
    }
    for note in faults {
        eprintln!("live: NOTE delivery never came up: {note}");
    }

    let quit_started = Instant::now();
    control
        .try_send(Control::Quit)
        .unwrap_or_else(|error| bail(&state_dir, &format!("Quit: {error}")));

    let shutdown = collect_until(
        &mut stream,
        &state_dir,
        QUIT_CAP,
        "Update::Stopped",
        |update| matches!(update, Update::Stopped),
    )
    .await;
    let quit_took = quit_started.elapsed();

    if !shutdown
        .iter()
        .any(|update| matches!(update, Update::Phase(Phase::ShuttingDown)))
    {
        bail(
            &state_dir,
            &format!("no Phase(ShuttingDown) before Stopped: {shutdown:?}"),
        );
    }
    if quit_took >= QUIT_BUDGET {
        bail(
            &state_dir,
            &format!(
                "shutdown took {quit_took:?} (budget {QUIT_BUDGET:?}); the \
                 logos thread was detached rather than joined"
            ),
        );
    }

    // Nothing may follow Stopped — the session pends forever, and the
    // stream is dropped right after this.
    if let Ok(Some(update)) =
        tokio::time::timeout(Duration::from_millis(200), stream.next()).await
    {
        bail(&state_dir, &format!("{update:?} followed Stopped"));
    }
    drop(stream);

    let deadline = Instant::now() + REAPED_CAP;
    let mut stray = stray_processes(&state_dir);
    while !stray.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        stray = stray_processes(&state_dir);
    }
    if !stray.is_empty() {
        bail(
            &state_dir,
            &format!("logoscore still holds {}:\n{stray}", state_dir.display()),
        );
    }

    eprintln!(
        "live: quit clean in {quit_took:?}; no process holds the config dir; \
         total {:?}",
        started.elapsed()
    );
}

/// The network-dependent leg, entered only once delivery is Online: create
/// a group, watch its `conversation_created` arrive through the real event
/// pump, then round-trip a member load for it.
async fn online_leg(
    stream: &mut (impl Stream<Item = Update> + Unpin),
    control: &mut futures::channel::mpsc::Sender<Control>,
    state_dir: &Path,
    started: Instant,
) {
    // Fresh-online always replays Ready + a resync snapshot first.
    collect_until(stream, state_dir, ACTION_CAP, "the resync snapshot", |u| {
        matches!(u, Update::ConversationsSnapshot(_))
    })
    .await;

    control
        .try_send(Control::CreateGroup {
            name: "frigicom live test".to_owned(),
            description: "created by tests/live.rs".to_owned(),
        })
        .unwrap_or_else(|error| {
            bail(state_dir, &format!("CreateGroup: {error}"))
        });

    let created = collect_until(
        stream,
        state_dir,
        ACTION_CAP,
        "Event(ConversationCreated)",
        |update| {
            matches!(
                update,
                Update::Event(ChatEvent::ConversationCreated { .. })
            )
        },
    )
    .await;
    let Some(Update::Event(ChatEvent::ConversationCreated {
        convo_id,
        is_outgoing,
        kind,
        ..
    })) = created.last()
    else {
        bail(state_dir, "the collected updates did not end in the event");
    };
    if !is_outgoing {
        bail(
            state_dir,
            &format!("{convo_id} came back is_outgoing=false, kind {kind:?}"),
        );
    }
    let convo_id = convo_id.clone();
    eprintln!(
        "live: create_group_conversation → {convo_id} ({kind:?}) at {:?}",
        started.elapsed()
    );

    control
        .try_send(Control::LoadMembers {
            convo_id: convo_id.clone(),
        })
        .unwrap_or_else(|error| {
            bail(state_dir, &format!("LoadMembers: {error}"))
        });

    let loaded = collect_until(
        stream,
        state_dir,
        ACTION_CAP,
        "MembersLoaded for the new group",
        |update| matches!(update, Update::MembersLoaded { convo_id: id, .. } if *id == convo_id),
    )
    .await;
    let Some(Update::MembersLoaded { members, .. }) = loaded.last() else {
        bail(
            state_dir,
            "the collected updates did not end in MembersLoaded",
        );
    };
    eprintln!(
        "live: list_group_members → {} member(s) at {:?}",
        members.len(),
        started.elapsed()
    );

    // The group has to survive a full refetch, not just the event pump:
    // `Resync` re-runs `list_conversations` against the module.
    control
        .try_send(Control::Resync)
        .unwrap_or_else(|error| bail(state_dir, &format!("Resync: {error}")));

    let resynced = collect_until(
        stream,
        state_dir,
        ACTION_CAP,
        "the post-create resync snapshot",
        |update| matches!(update, Update::ConversationsSnapshot(_)),
    )
    .await;
    let Some(Update::ConversationsSnapshot(conversations)) = resynced.last()
    else {
        bail(state_dir, "the collected updates did not end in a snapshot");
    };
    if !conversations
        .iter()
        .any(|conversation| conversation.convo_id == convo_id)
    {
        bail(
            state_dir,
            &format!(
                "list_conversations lost {convo_id}; got {:?}",
                conversations
                    .iter()
                    .map(|conversation| &conversation.convo_id)
                    .collect::<Vec<_>>()
            ),
        );
    }
    eprintln!(
        "live: list_conversations contains {convo_id} ({} total) at {:?}",
        conversations.len(),
        started.elapsed()
    );
}

/// The very first item, which the session emits before touching the daemon.
async fn first(
    stream: &mut (impl Stream<Item = Update> + Unpin),
    state_dir: &Path,
) -> Update {
    next(
        stream,
        state_dir,
        Duration::from_secs(10),
        "Update::Controller",
    )
    .await
}

async fn next(
    stream: &mut (impl Stream<Item = Update> + Unpin),
    state_dir: &Path,
    within: Duration,
    what: &str,
) -> Update {
    match tokio::time::timeout(within, stream.next()).await {
        Ok(Some(update)) => update,
        Ok(None) => {
            bail(state_dir, &format!("stream ended waiting for {what}"))
        }
        Err(_) => bail(
            state_dir,
            &format!("timed out after {within:?} waiting for {what}"),
        ),
    }
}

/// [`pump_until`] with the deadline treated as a failure.
async fn collect_until(
    stream: &mut (impl Stream<Item = Update> + Unpin),
    state_dir: &Path,
    within: Duration,
    what: &str,
    pred: impl Fn(&Update) -> bool,
) -> Vec<Update> {
    match pump_until(stream, state_dir, within, pred).await {
        Some(updates) => updates,
        None => bail(
            state_dir,
            &format!("timed out after {within:?} waiting for {what}"),
        ),
    }
}

/// Pumps the session until `pred` matches, returning everything received
/// including the match. `None` means the window closed first — the caller
/// decides whether that is a failure. A `Fatal` always aborts the run: the
/// session pends forever after it, so waiting on anything else would only
/// burn the timeout.
async fn pump_until(
    stream: &mut (impl Stream<Item = Update> + Unpin),
    state_dir: &Path,
    within: Duration,
    pred: impl Fn(&Update) -> bool,
) -> Option<Vec<Update>> {
    let deadline = Instant::now() + within;
    let mut updates = Vec::new();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Update::Fatal(error))) => bail(
                state_dir,
                &format!("session gave up: {error} (after {updates:?})"),
            ),
            Ok(Some(update)) => {
                let done = pred(&update);
                updates.push(update);
                if done {
                    return Some(updates);
                }
            }
            Ok(None) => bail(state_dir, "the update stream ended"),
            Err(_) => return None,
        }
    }
}

fn phases(updates: &[Update]) -> Vec<Phase> {
    updates
        .iter()
        .filter_map(|update| match update {
            Update::Phase(phase) => Some(phase.clone()),
            _ => None,
        })
        .collect()
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from)
}

/// Lines where the delivery module said it could not come up. Empty when
/// it started cleanly.
fn delivery_faults(state_dir: &Path) -> Vec<String> {
    let log = state_dir.join("logoscore").join("logoscore.log");
    let Ok(contents) = std::fs::read_to_string(log) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter(|line| {
            line.contains("createNode callback error")
                || line.contains("Cannot start Delivery")
        })
        .map(str::trim)
        .map(str::to_owned)
        .collect()
}

/// Processes whose command line still mentions our state dir — after a
/// clean quit there must be none.
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

/// Fails with the daemon's own log attached: the temp dir goes away as the
/// panic unwinds, so this is the only chance to read it.
fn bail(state_dir: &Path, message: &str) -> ! {
    let log = state_dir.join("logoscore").join("logoscore.log");
    let tail = std::fs::read_to_string(&log).map_or_else(
        |error| format!("({}: {error})", log.display()),
        |contents| {
            let lines: Vec<&str> = contents.lines().collect();
            lines[lines.len().saturating_sub(LOG_TAIL_LINES)..].join("\n")
        },
    );
    panic!("{message}\n--- tail of {} ---\n{tail}", log.display());
}
