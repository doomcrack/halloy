use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Logos {
    /// Passed to `chat_module.init()`.
    pub delivery_preset: String,
    /// Overrides the bundled/`PATH` lookup for the `logoscore` binary.
    pub daemon_path: Option<PathBuf>,
    /// Overrides the staged module artifacts directory.
    pub modules_dir: Option<PathBuf>,
    /// Overrides the daemon's state/config directory.
    pub instance_dir: Option<PathBuf>,
    pub installation_name: Option<String>,
    /// Watch all chat events with one wildcard call instead of seven
    /// explicit ones. The QtRO relay leg is unverified for wildcards —
    /// leave off unless tested against the pinned daemon.
    pub use_wildcard_watch: bool,
    /// Run against the scripted in-process mock instead of a live
    /// daemon. The live daemon is the default; the mock exists to drive
    /// the UI without logoscore artifacts (and is the only backend a
    /// `--no-default-features` build can reach).
    pub mock: bool,
}

impl Default for Logos {
    fn default() -> Self {
        Self {
            delivery_preset: "logos.dev".to_owned(),
            daemon_path: None,
            modules_dir: None,
            instance_dir: None,
            installation_name: None,
            use_wildcard_watch: false,
            mock: false,
        }
    }
}
