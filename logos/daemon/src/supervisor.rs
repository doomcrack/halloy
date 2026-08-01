//! Spawn / health / stop / stale-reap of the app-owned logoscore daemon.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::Instant;

use crate::{Artifacts, DaemonError, Supervisor, process, state_json, token};

const HEALTHY_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(150);
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const KILL_GRACE: Duration = Duration::from_secs(2);
/// Slack for the bootstrap token file, which the daemon writes around the
/// same time as `state.json`.
const TOKEN_TIMEOUT: Duration = Duration::from_secs(5);
/// Every invocation carries it, which is what makes a daemon's argv proof
/// of which config dir it holds.
const CONFIG_DIR_FLAG: &str = "--config-dir";
/// The daemon's combined log: its own output plus every module's stdout and
/// stderr, in one file inside the config dir.
const LOG_FILE_NAME: &str = "logoscore.log";

/// Where [`start`] points the daemon's stdout and stderr. Exposed so readers
/// (the module monitor tails it) resolve the same file the supervisor
/// creates, instead of growing a second copy of the naming rule.
pub fn log_path(config_dir: &Path) -> PathBuf {
    config_dir.join(LOG_FILE_NAME)
}

pub async fn start(
    artifacts: &Artifacts,
    config_dir: &Path,
) -> Result<Supervisor, DaemonError> {
    tokio::fs::create_dir_all(config_dir).await?;

    // Daemon output goes to a file: piping without draining could block
    // the child once the pipe buffer fills over a long session.
    let log_path = log_path(config_dir);
    let log = std::fs::File::create(&log_path)?;

    let mut child = Command::new(&artifacts.logoscore_bin)
        .arg("-D")
        .arg("-m")
        .arg(&artifacts.modules_dir)
        .arg(CONFIG_DIR_FLAG)
        .arg(config_dir)
        .args([
            "--module-transport",
            "core_service=tcp,host=127.0.0.1,port=0",
        ])
        .args([
            "--module-transport",
            "capability_module=tcp,host=127.0.0.1,port=0",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| DaemonError::Spawn(error.to_string()))?;

    let state_json_path = state_json::path_in(config_dir);
    let deadline = Instant::now() + HEALTHY_TIMEOUT;
    let child_pid = child.id();

    let ports = loop {
        if let Some(status) = child.try_wait()? {
            return Err(DaemonError::Spawn(format!(
                "logoscore exited during startup ({status}): {}",
                log_tail(&log_path)
            )));
        }

        // port=0 auto-assigns; state.json appears once the daemon resolved
        // and bound its listeners. A crashed prior run leaves a stale
        // state.json behind, so only a file naming the spawned child's pid
        // is trusted — otherwise the status probe could see the fresh
        // daemon's file while the ports came from the dead one's. Confirm
        // RPC-level health before declaring victory.
        if let Some(pid) = child_pid
            && let Ok(ports) =
                state_json::read_ports_for_pid(&state_json_path, pid)
            && status_ok(&artifacts.logoscore_bin, config_dir).await
        {
            break ports;
        }

        if Instant::now() >= deadline {
            let _ = child.kill().await;
            return Err(DaemonError::Unhealthy(HEALTHY_TIMEOUT));
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    };

    let token = token::read_bootstrap_token(config_dir, TOKEN_TIMEOUT).await?;

    Ok(Supervisor {
        child,
        config_dir: config_dir.to_path_buf(),
        logoscore_bin: artifacts.logoscore_bin.clone(),
        ports,
        token,
    })
}

pub async fn is_alive(supervisor: &mut Supervisor) -> bool {
    supervisor
        .child
        .try_wait()
        .is_ok_and(|status| status.is_none())
}

pub async fn stop(supervisor: Supervisor, grace: Duration) {
    let Supervisor {
        mut child,
        config_dir,
        logoscore_bin,
        ..
    } = supervisor;

    log::debug!(
        "stopping logoscore ({}) for {}",
        logoscore_bin.display(),
        config_dir.display()
    );

    if tokio::time::timeout(grace, child.wait()).await.is_ok() {
        return;
    }

    if let Some(pid) = child.id() {
        // SIGTERM first: the daemon installs handlers for a clean shutdown
        // (removes state.json, unlinks sockets).
        process::terminate(i64::from(pid));
    }

    if tokio::time::timeout(KILL_GRACE, child.wait()).await.is_ok() {
        return;
    }

    let _ = child.kill().await;
}

pub async fn reap_stale(
    artifacts: &Artifacts,
    config_dir: &Path,
) -> Result<(), DaemonError> {
    let state_json = state_json::path_in(config_dir);

    // A missing or unparsable state.json means nothing to reap; the
    // daemon overwrites stale state itself.
    let Ok(pid) = state_json::read_pid(&state_json) else {
        return Ok(());
    };

    if pid <= 1 || !process::alive(pid) {
        return Ok(());
    }

    // The recorded pid is a hint, never a licence to signal: only a clean
    // exit removes state.json, so after a crash plus a reboot — or enough
    // process churn — the number names an unrelated process the user owns.
    // Nothing is signalled unless it can be identified as logoscore
    // holding THIS config dir; anything else means the file is stale, and
    // dropping it is what unblocks the daemon's duplicate-launch guard.
    if !is_our_daemon(pid, artifacts, config_dir).await {
        log::warn!(
            "{} names pid {pid}, which is not a logoscore daemon for {}; \
             discarding the stale file rather than signalling that process",
            state_json.display(),
            config_dir.display()
        );

        return discard(&state_json);
    }

    log::warn!(
        "stale logoscore daemon (pid {pid}) holds {}; asking it to stop",
        config_dir.display()
    );

    let stop = Command::new(&artifacts.logoscore_bin)
        .arg(CONFIG_DIR_FLAG)
        .arg(config_dir)
        .arg("stop")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match tokio::time::timeout(STOP_TIMEOUT, stop).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => log::warn!("could not run logoscore stop: {error}"),
        Err(_) => log::warn!("logoscore stop timed out"),
    }

    if process::wait_for_death(pid, KILL_GRACE).await {
        return Ok(());
    }

    process::kill(pid);

    if process::wait_for_death(pid, KILL_GRACE).await {
        return Ok(());
    }

    Err(DaemonError::Spawn(format!(
        "stale daemon (pid {pid}) survived SIGKILL; config dir {} is still held",
        config_dir.display()
    )))
}

/// Whether `pid` is a logoscore invocation pointed at `config_dir`. Every
/// spawn names the binary and passes `--config-dir <config_dir>`, so both
/// show up in the process's own argv. Anything unreadable counts as "not
/// ours": no process may be signalled on a maybe.
async fn is_our_daemon(
    pid: i64,
    artifacts: &Artifacts,
    config_dir: &Path,
) -> bool {
    let (Some(program), Some(dir)) =
        (artifacts.logoscore_bin.file_name(), config_dir.to_str())
    else {
        return false;
    };

    let Some(argv) = process::argv(pid).await else {
        return false;
    };

    // Flag and value as one substring: a config dir with spaces in it
    // survives (macOS puts ours under `Application Support`), and a
    // process that merely mentions the directory — a tail on the daemon
    // log, an editor — does not pass for having the path in its argv.
    if !argv.contains(&format!("{CONFIG_DIR_FLAG} {dir}")) {
        return false;
    }

    // The program is argv[0]; a script shows its interpreter first and
    // itself second. Looking no further keeps a path *argument* that
    // happens to end in `logoscore` from vouching for the process.
    argv.split_whitespace()
        .take(2)
        .any(|token| Path::new(token).file_name() == Some(program))
}

/// Drops a state.json that describes no live daemon of ours, so the guard
/// inside the freshly spawned logoscore does not read it as a duplicate
/// launch. A file that vanished under us is the outcome we wanted.
fn discard(state_json: &Path) -> Result<(), DaemonError> {
    match std::fs::remove_file(state_json) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DaemonError::State(format!(
            "could not discard stale {}: {error}",
            state_json.display()
        ))),
    }
}

async fn status_ok(logoscore_bin: &Path, config_dir: &Path) -> bool {
    let status = Command::new(logoscore_bin)
        .arg(CONFIG_DIR_FLAG)
        .arg(config_dir)
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    matches!(
        tokio::time::timeout(STATUS_TIMEOUT, status).await,
        Ok(Ok(status)) if status.success()
    )
}

fn log_tail(log_path: &Path) -> String {
    let Ok(contents) = std::fs::read_to_string(log_path) else {
        return String::new();
    };

    let lines: Vec<&str> = contents.lines().collect();
    let tail = lines.len().saturating_sub(10);

    lines[tail..].join("\n")
}
