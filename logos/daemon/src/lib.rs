//! Supervision of a private `logoscore` daemon: locating artifacts, spawning
//! a fresh daemon per app run, issuing an auth token, discovering the TCP
//! ports it bound, probing liveness, and stopping it.
//!
//! Ground truth (verified against the pinned logos-logoscore-cli source):
//! - Spawn: `logoscore -D -m <modules_dir> --config-dir <dir>
//!   --module-transport core_service=tcp,host=127.0.0.1,port=0
//!   --module-transport capability_module=tcp,host=127.0.0.1,port=0`.
//!   `-D` runs in the FOREGROUND (callers background it themselves) — the
//!   spawned child IS the daemon; hold the `Child`.
//! - Both well-known modules need TCP listeners: a Qt-free client cannot use
//!   the implicit LocalSocket listeners, and the capability_module dial is
//!   part of every first call.
//! - A LocalSocket listener is always implicitly prepended per module;
//!   plaintext tcp on loopback needs no `--insecure-tcp`.
//! - Duplicate-launch guard: the daemon refuses to start when
//!   `<configDir>/daemon/state.json` names a live pid. [`reap_stale`] clears
//!   a leftover daemon from a crashed prior run before spawning — but only
//!   one it can identify as logoscore; see [`reap_stale`].
//! - Ports: `port=0` auto-assigns; the ACTUAL bound ports are read from
//!   `<configDir>/daemon/state.json` under
//!   `resolved.modules.<name>.transports[]` (entries `{protocol, host,
//!   port, codec, ...}`; pick `protocol == "tcp"`). Poll until the file
//!   exists and carries non-zero tcp ports for both modules.
//! - Token: the daemon mints its own bootstrap client token at startup and
//!   writes it to `<configDir>/client/<token_file>` (named by
//!   `<configDir>/client/config.json`, default `auto.json`). That is the
//!   ONLY token the pinned daemon's auth gate accepts — tokens minted with
//!   `issue-token --name <name>` are registered but refused on every call
//!   ("auth token not recognized"); see [`token`] for the live evidence.
//! - Liveness: `logoscore --config-dir <dir> status --json` exits 0 when
//!   running (the RPC is the probe); also `child.try_wait()` for cheap
//!   process-level checks. Prefer the process-level check on the hot path —
//!   it works even while an lp call is blocked.
//! - Shutdown: the session layer calls `core_service.shutdown` first (the
//!   daemon quits ~200ms later); [`Supervisor::stop`] then waits for child
//!   exit with a grace period, escalating SIGTERM → SIGKILL.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Child;

/// Origin module name the app presents to the daemon on every call. Not a
/// token name: the daemon authenticates with its own bootstrap token (see
/// [`token`]) and never checks the origin against it.
pub const TOKEN_NAME: &str = "frigicom";

/// Optional explicit locations, taking precedence over environment lookup.
#[derive(Debug, Clone, Default)]
pub struct ArtifactOverrides {
    pub logoscore_bin: Option<PathBuf>,
    pub modules_dir: Option<PathBuf>,
}

/// Resolved on-disk artifacts required to run the daemon.
#[derive(Debug, Clone)]
pub struct Artifacts {
    pub logoscore_bin: PathBuf,
    pub modules_dir: PathBuf,
}

/// Actual loopback TCP ports the daemon bound for the two well-known
/// modules, discovered from `daemon/state.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    pub core_service: u16,
    pub capability_module: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Artifact {
    LogoscoreBinary,
    ModulesDir,
}

#[derive(Debug, thiserror::Error)]
pub enum LocateError {
    #[error("could not locate {artifact:?}; searched {searched:?}")]
    NotFound {
        artifact: Artifact,
        searched: Vec<PathBuf>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("failed to spawn logoscore: {0}")]
    Spawn(String),
    #[error("daemon did not become healthy within {0:?}")]
    Unhealthy(Duration),
    #[error("token issuance failed: {0}")]
    Token(String),
    #[error("could not read daemon state: {0}")]
    State(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Locates the `logoscore` binary and the staged modules directory.
/// Resolution order per artifact: explicit override → environment
/// (`LOGOSCORE_BIN` / `LOGOS_MODULES_DIR`) → the bundle beside the running
/// executable (`logos/bin/logoscore` + `logos/modules`, or the same under
/// `../Resources` in a macOS .app) → `PATH` lookup (binary only). Reports
/// every searched location on failure.
pub fn locate(overrides: &ArtifactOverrides) -> Result<Artifacts, LocateError> {
    locate::locate(overrides)
}

/// Clears a stale daemon left by a crashed prior run: if
/// `<config_dir>/daemon/state.json` names a pid that is still alive AND
/// that pid is a logoscore invocation pointed at this config dir, ask it to
/// stop (`logoscore --config-dir <dir> stop`), then escalate to SIGKILL
/// after a short grace. A dead recorded pid is ignored (the daemon
/// overwrites stale state itself); a live one that cannot be identified is
/// never signalled — the recorded number may have been recycled by an
/// unrelated process — the stale file is discarded instead.
pub async fn reap_stale(
    artifacts: &Artifacts,
    config_dir: &Path,
) -> Result<(), DaemonError> {
    supervisor::reap_stale(artifacts, config_dir).await
}

/// A running, app-owned logoscore daemon.
#[derive(Debug)]
pub struct Supervisor {
    child: Child,
    config_dir: PathBuf,
    logoscore_bin: PathBuf,
    ports: Ports,
    token: String,
}

impl Supervisor {
    /// Spawns a fresh daemon against `config_dir`, waits for it to become
    /// healthy (bounded), discovers the bound TCP ports, and reads the
    /// bootstrap token it minted for its clients.
    pub async fn start(
        artifacts: &Artifacts,
        config_dir: &Path,
    ) -> Result<Self, DaemonError> {
        supervisor::start(artifacts, config_dir).await
    }

    pub fn ports(&self) -> Ports {
        self.ports
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Cheap process-level liveness: has the child exited? Deliberately not
    /// an RPC — must stay responsive while an lp call is blocked.
    pub async fn is_alive(&mut self) -> bool {
        supervisor::is_alive(self).await
    }

    /// Waits up to `grace` for the child to exit on its own (the session
    /// layer already requested `core_service.shutdown`), then SIGTERM, short
    /// wait, then SIGKILL. Removes nothing on disk.
    pub async fn stop(self, grace: Duration) {
        supervisor::stop(self, grace).await;
    }
}

mod locate;
mod process;
mod state_json;
mod supervisor;
mod token;

pub use state_json::read_ports;
