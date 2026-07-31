//! Liveness, identity and signalling for a pid the supervisor did not
//! spawn (a leftover daemon recorded in `daemon/state.json`). All of it is
//! POSIX; elsewhere the answers are the conservative ones — nothing is
//! alive, nothing is identifiable, nothing is signalled — which is honest,
//! since the live backend needs `liblogos_protocol` and that ships for
//! macOS and Linux only.

use std::time::Duration;

use tokio::time::Instant;

const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Whether `pid` still names a live process.
#[cfg(unix)]
pub fn alive(pid: i64) -> bool {
    signal(pid, 0)
}

#[cfg(not(unix))]
pub fn alive(_pid: i64) -> bool {
    false
}

/// Asks `pid` to shut down cleanly. The daemon handles SIGTERM by removing
/// its state.json and unlinking its sockets.
#[cfg(unix)]
pub fn terminate(pid: i64) {
    let _ = signal(pid, libc::SIGTERM);
}

#[cfg(not(unix))]
pub fn terminate(_pid: i64) {}

/// Ends `pid` without giving it a say.
#[cfg(unix)]
pub fn kill(pid: i64) {
    let _ = signal(pid, libc::SIGKILL);
}

#[cfg(not(unix))]
pub fn kill(_pid: i64) {}

pub async fn wait_for_death(pid: i64, within: Duration) -> bool {
    let deadline = Instant::now() + within;

    while alive(pid) {
        if Instant::now() >= deadline {
            return false;
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }

    true
}

/// The command line of `pid` as it was passed, or `None` when it cannot be
/// read. macOS has no `/proc`, so `ps` is the portable reader; `-ww` stops
/// it truncating the arguments the caller needs to see.
#[cfg(unix)]
pub async fn argv(pid: i64) -> Option<String> {
    /// `ps` is a fork+exec on the startup path: it answers in
    /// milliseconds or it is not going to.
    const TIMEOUT: Duration = Duration::from_secs(2);

    let output = tokio::process::Command::new("ps")
        .args(["-ww", "-o", "args=", "-p"])
        .arg(pid.to_string())
        .output();

    let Ok(Ok(output)) = tokio::time::timeout(TIMEOUT, output).await else {
        return None;
    };

    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(not(unix))]
pub async fn argv(_pid: i64) -> Option<String> {
    None
}

#[cfg(unix)]
fn signal(pid: i64, signal: libc::c_int) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };

    unsafe { libc::kill(pid, signal) == 0 }
}
