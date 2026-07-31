//! Session configuration, built by the application from its own config
//! surface (`[logos]` in config.toml) plus platform paths.

use std::path::PathBuf;

use logos_daemon::ArtifactOverrides;

#[derive(Debug, Clone)]
pub struct BackendConfig {
    pub artifacts: ArtifactOverrides,
    /// App data dir; the daemon's config dir is `<state_dir>/logoscore`.
    pub state_dir: PathBuf,
    /// Passed to `chat_module.init()`. Empty selects the module default.
    pub delivery_preset: String,
    /// Applied via `set_installation_name` after each successful init;
    /// `None` leaves the module's stored name untouched.
    pub installation_name: Option<String>,
    /// Origin module name presented to the daemon on every call. Purely
    /// descriptive — the daemon authenticates on its bootstrap token alone.
    pub origin: String,
    /// Watch all chat events with one wildcard call instead of seven
    /// explicit ones. The QtRO relay leg is unverified for wildcards —
    /// leave off unless tested against the pinned daemon.
    pub use_wildcard_watch: bool,
    /// Whether a leftover daemon holding the config dir may be cleared
    /// before spawning. The app turns this off when it could not take the
    /// single-instance guard: the daemon `daemon/state.json` describes may
    /// then belong to a live sibling instance, and reaping it would pull
    /// the backend out from under a running app.
    pub reap_stale_daemon: bool,
    pub restart_max_attempts: u32,
}

impl BackendConfig {
    pub fn new(state_dir: PathBuf) -> Self {
        Self {
            artifacts: ArtifactOverrides::default(),
            state_dir,
            delivery_preset: "logos.dev".to_owned(),
            installation_name: None,
            origin: logos_daemon::TOKEN_NAME.to_owned(),
            use_wildcard_watch: false,
            reap_stale_daemon: true,
            restart_max_attempts: 3,
        }
    }

    pub fn daemon_config_dir(&self) -> PathBuf {
        self.state_dir.join("logoscore")
    }
}
