//! Supervisor lifecycle tests driven by a stub shell script that mimics
//! logoscore: `-D` writes a fake state.json plus the bootstrap client token
//! (`client/config.json` + `client/auto.json`, as the real daemon does) and
//! sleeps, `status --json` exits 0 while state.json exists, `stop` SIGTERMs
//! the pid recorded in state.json. An optional live smoke test against the
//! real binary runs only with `LOGOS_LIVE_TESTS=1`.
//!
//! The stub is a shell script and the assertions signal pids directly, so
//! the file is unix-only — as is the supervision it covers.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use logos_daemon::{
    ArtifactOverrides, Artifacts, DaemonError, Supervisor, locate, reap_stale,
};

const STUB: &str = r##"#!/bin/sh
config_dir=""
subcommand=""
daemon=0

while [ $# -gt 0 ]; do
    case "$1" in
        --config-dir) config_dir="$2"; shift 2 ;;
        -D) daemon=1; shift ;;
        -m|--modules-dir) shift 2 ;;
        --module-transport) shift 2 ;;
        --json|-j) shift ;;
        status|stop) subcommand="$1"; shift ;;
        *) shift ;;
    esac
done

state="$config_dir/daemon/state.json"

if [ "$daemon" = 1 ]; then
    STUB_DAEMON_HOOK
    mkdir -p "$config_dir/daemon"
    trap 'rm -f "$state"; exit 0' TERM INT
    sleep 0.2
    cat > "$state" <<EOF
{
  "version": 2,
  "instance_id": "stub00000000",
  "pid": $$,
  "started_at": "2026-01-01T00:00:00Z",
  "resolved": {
    "modules": {
      "core_service": {
        "transports": [
          { "protocol": "local" },
          { "protocol": "tcp", "host": "127.0.0.1", "port": 41001, "codec": "json" }
        ]
      },
      "capability_module": {
        "transports": [
          { "protocol": "local" },
          { "protocol": "tcp", "host": "127.0.0.1", "port": 41002, "codec": "json" }
        ]
      }
    }
  }
}
EOF
    mkdir -p "$config_dir/client"
    printf '{"version":2,"instance_id":"stub00000000","token_file":"auto.json"}\n' > "$config_dir/client/config.json"
    printf '{"version":1,"name":"auto","token":"stub-bootstrap-token","issued_at":"2026-01-01T00:00:00Z"}\n' > "$config_dir/client/auto.json"
    while :; do sleep 0.1; done
fi

case "$subcommand" in
    status)
        [ -f "$state" ] || exit 1
        echo '{"daemon":{"status":"running"}}'
        exit 0
        ;;
    stop)
        pid=$(sed -n 's/.*"pid": \([0-9]*\).*/\1/p' "$state" 2>/dev/null)
        [ -n "$pid" ] && kill -TERM "$pid" 2>/dev/null
        exit 0
        ;;
esac

exit 64
"##;

struct Fixture {
    _root: tempfile::TempDir,
    artifacts: Artifacts,
    config_dir: PathBuf,
}

fn fixture_with(daemon_hook: &str) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("logoscore");
    let modules = root.path().join("modules");
    let config_dir = root.path().join("config");

    fs::write(&bin, STUB.replace("STUB_DAEMON_HOOK", daemon_hook)).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir(&modules).unwrap();

    Fixture {
        artifacts: Artifacts {
            logoscore_bin: bin,
            modules_dir: modules,
        },
        config_dir,
        _root: root,
    }
}

fn fixture() -> Fixture {
    fixture_with("")
}

fn pid_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

async fn wait_for<F: Fn() -> bool>(what: &str, condition: F) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);

    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn start_discovers_ports_reads_token_and_stops_cleanly() {
    let fixture = fixture();

    let mut supervisor =
        Supervisor::start(&fixture.artifacts, &fixture.config_dir)
            .await
            .unwrap();

    assert_eq!(supervisor.ports().core_service, 41001);
    assert_eq!(supervisor.ports().capability_module, 41002);
    assert_eq!(supervisor.token(), "stub-bootstrap-token");
    assert!(supervisor.is_alive().await);

    let pid = supervisor.pid().unwrap();
    let state_json = fixture.config_dir.join("daemon").join("state.json");
    assert!(state_json.is_file());

    // The stub won't exit within grace, so stop() escalates to SIGTERM;
    // the stub's TERM trap removes state.json — proof of a clean shutdown.
    supervisor.stop(Duration::from_millis(200)).await;

    wait_for("daemon death", || !pid_alive(pid)).await;
    assert!(!state_json.exists());
}

#[tokio::test]
async fn start_ignores_stale_state_json_from_a_crashed_run() {
    let fixture = fixture();

    // A SIGKILLed prior daemon leaves its state.json behind (only a clean
    // exit removes it). The stub daemon takes ~200ms to write its own file
    // while `status` already succeeds on the stale one — start() must wait
    // for the file naming the spawned child's pid, not grab the dead
    // daemon's ports.
    let mut dead = std::process::Command::new("true").spawn().unwrap();
    let dead_pid = dead.id();
    dead.wait().unwrap();

    let daemon_dir = fixture.config_dir.join("daemon");
    fs::create_dir_all(&daemon_dir).unwrap();
    fs::write(
        daemon_dir.join("state.json"),
        format!(
            r#"{{"version":2,"instance_id":"stale","pid":{dead_pid},"resolved":{{"modules":{{"core_service":{{"transports":[{{"protocol":"tcp","port":59001}}]}},"capability_module":{{"transports":[{{"protocol":"tcp","port":59002}}]}}}}}}}}"#
        ),
    )
    .unwrap();

    let supervisor = Supervisor::start(&fixture.artifacts, &fixture.config_dir)
        .await
        .unwrap();

    assert_eq!(supervisor.ports().core_service, 41001);
    assert_eq!(supervisor.ports().capability_module, 41002);

    supervisor.stop(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn stop_escalates_to_sigkill_when_sigterm_is_ignored() {
    let fixture = fixture_with("trap '' TERM");

    let supervisor = Supervisor::start(&fixture.artifacts, &fixture.config_dir)
        .await
        .unwrap();
    let pid = supervisor.pid().unwrap();

    supervisor.stop(Duration::from_millis(100)).await;

    assert!(!pid_alive(pid));
}

#[tokio::test]
async fn start_fails_fast_when_daemon_exits() {
    let fixture = fixture_with(
        "echo 'Error: a logoscore daemon is already running in this config dir' >&2; exit 1",
    );

    let error = Supervisor::start(&fixture.artifacts, &fixture.config_dir)
        .await
        .unwrap_err();

    let DaemonError::Spawn(message) = error else {
        panic!("expected Spawn error, got {error}");
    };
    assert!(message.contains("exited during startup"));
    assert!(message.contains("already running"));
}

#[tokio::test]
async fn locate_finds_stub_via_overrides() {
    let fixture = fixture();

    let artifacts = locate(&ArtifactOverrides {
        logoscore_bin: Some(fixture.artifacts.logoscore_bin.clone()),
        modules_dir: Some(fixture.artifacts.modules_dir.clone()),
    })
    .unwrap();

    assert_eq!(artifacts.logoscore_bin, fixture.artifacts.logoscore_bin);
}

#[tokio::test]
async fn reap_stale_stops_live_leftover_daemon() {
    let fixture = fixture();

    // Simulate a crashed prior app run: a daemon we no longer supervise.
    let mut leftover =
        tokio::process::Command::new(&fixture.artifacts.logoscore_bin)
            .arg("--config-dir")
            .arg(&fixture.config_dir)
            .arg("-D")
            .kill_on_drop(true)
            .spawn()
            .unwrap();

    let state_json = fixture.config_dir.join("daemon").join("state.json");
    wait_for("state.json", || state_json.is_file()).await;

    // Reap the zombie as soon as the leftover dies, like init would for a
    // reparented orphan; reap_stale probes liveness with kill(pid, 0).
    let pid = leftover.id().unwrap();
    tokio::spawn(async move {
        let _ = leftover.wait().await;
    });

    reap_stale(&fixture.artifacts, &fixture.config_dir)
        .await
        .unwrap();

    assert!(!pid_alive(pid));
    // The stub's TERM trap removed state.json — the polite path worked.
    assert!(!state_json.exists());
}

#[tokio::test]
async fn reap_stale_ignores_dead_and_missing_pids() {
    let fixture = fixture();

    // No state.json at all.
    reap_stale(&fixture.artifacts, &fixture.config_dir)
        .await
        .unwrap();

    // state.json recording a pid that is no longer alive.
    let mut dead = std::process::Command::new("true").spawn().unwrap();
    let dead_pid = dead.id();
    dead.wait().unwrap();

    let daemon_dir = fixture.config_dir.join("daemon");
    fs::create_dir_all(&daemon_dir).unwrap();
    fs::write(
        daemon_dir.join("state.json"),
        format!(r#"{{"version":2,"instance_id":"stale","pid":{dead_pid}}}"#),
    )
    .unwrap();

    reap_stale(&fixture.artifacts, &fixture.config_dir)
        .await
        .unwrap();
}

#[tokio::test]
async fn reap_stale_never_signals_a_pid_that_is_not_logoscore() {
    let fixture = fixture();

    let daemon_dir = fixture.config_dir.join("daemon");
    fs::create_dir_all(&daemon_dir).unwrap();
    let state_json = daemon_dir.join("state.json");
    fs::write(&state_json, r#"{"version":2,"pid":1}"#).unwrap();

    // Only a clean exit removes state.json, so a recorded pid outlives the
    // daemon and eventually names something else entirely. This bystander
    // even has the config dir in its own command line — someone watching
    // the daemon's state — which must not be enough to pass for the
    // daemon.
    let mut bystander = tokio::process::Command::new("tail")
        .arg("-f")
        .arg(&state_json)
        .stdout(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = bystander.id().unwrap();

    fs::write(
        &state_json,
        format!(r#"{{"version":2,"instance_id":"recycled","pid":{pid}}}"#),
    )
    .unwrap();

    reap_stale(&fixture.artifacts, &fixture.config_dir)
        .await
        .unwrap();

    assert!(pid_alive(pid), "reap_stale signalled an unrelated process");
    // It described no daemon of ours, so it must not block the next spawn
    // through the daemon's own duplicate-launch guard either.
    assert!(!state_json.exists());

    let _ = bystander.kill().await;
}

// The identity check gates every reap, so it has to accept the process the
// real logoscore actually becomes — not just the stub. Same opt-in gate as
// the roundtrip below.
#[tokio::test]
async fn live_reap_stale_clears_a_real_leftover_daemon() {
    if std::env::var("LOGOS_LIVE_TESTS").as_deref() != Ok("1") {
        return;
    }

    let artifacts = locate(&ArtifactOverrides::default()).unwrap();
    let config_root = tempfile::tempdir().unwrap();
    let config_dir = config_root.path().join("config");

    // A daemon nobody supervises any more: what a crashed app run leaves.
    let mut leftover = tokio::process::Command::new(&artifacts.logoscore_bin)
        .arg("-D")
        .arg("-m")
        .arg(&artifacts.modules_dir)
        .arg("--config-dir")
        .arg(&config_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let state_json = config_dir.join("daemon").join("state.json");
    wait_for("live state.json", || state_json.is_file()).await;

    let pid = leftover.id().unwrap();
    tokio::spawn(async move {
        let _ = leftover.wait().await;
    });

    reap_stale(&artifacts, &config_dir).await.unwrap();

    wait_for("live leftover death", || !pid_alive(pid)).await;
}

// Live smoke test against the real logoscore binary; opt-in because it
// binds sockets and spawns module hosts. Run with:
// LOGOS_LIVE_TESTS=1 LOGOSCORE_BIN=... LOGOS_MODULES_DIR=... cargo test
#[tokio::test]
async fn live_supervisor_roundtrip() {
    if std::env::var("LOGOS_LIVE_TESTS").as_deref() != Ok("1") {
        return;
    }

    let artifacts = locate(&ArtifactOverrides::default()).unwrap();
    let config_root = tempfile::tempdir().unwrap();
    let config_dir = config_root.path().join("config");

    reap_stale(&artifacts, &config_dir).await.unwrap();

    let mut supervisor =
        Supervisor::start(&artifacts, &config_dir).await.unwrap();

    assert_ne!(supervisor.ports().core_service, 0);
    assert_ne!(supervisor.ports().capability_module, 0);
    assert_ne!(
        supervisor.ports().core_service,
        supervisor.ports().capability_module
    );
    assert!(!supervisor.token().is_empty());
    assert!(supervisor.is_alive().await);

    let pid = supervisor.pid().unwrap();
    supervisor.stop(Duration::from_millis(500)).await;

    wait_for("live daemon death", || !pid_alive(pid)).await;
}
