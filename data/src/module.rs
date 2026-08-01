//! Logos modules as frigicom sees them: the daemon-side plugins
//! (`chat_module`, `delivery_module`, …) whose lifecycle and logs the module
//! monitor renders. A module is identified everywhere by its wire name, the
//! same string the daemon accepts in `load-module`/`call` and stamps on every
//! log line, so `ModuleId` doubles as the log-attribution key and the buffer
//! key.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub mod log;
pub mod tail;

/// The daemon itself. It is not a loadable module and will never appear in
/// `listModules`, but its own log lines carry no module tag and would
/// otherwise be dropped, so they are routed to this id.
pub const DAEMON: &str = "logoscore";

/// The module whose dependency closure must stay loaded for chat to work.
const CHAT: &str = "chat_module";

/// The module set frigicom ships, and the only one it will ever address.
///
/// Hardcoded on purpose: there is no user-supplied module support, and
/// `listModules` is a status source, not a discovery mechanism. Versions and
/// dependency lists are the ones verified against the staged build — chat
/// declares `delivery_module` and auto-loads it, blockchain declares nothing.
const STAGED: &[Staged] = &[
    Staged {
        id: CHAT,
        version: "0.2.1",
        dependencies: &["delivery_module"],
    },
    Staged {
        id: "delivery_module",
        version: "0.1.3",
        dependencies: &[],
    },
    Staged {
        id: "capability_module",
        version: "1.0.0",
        dependencies: &[],
    },
    Staged {
        id: "blockchain_module",
        version: "0.0.999",
        dependencies: &[],
    },
];

/// A module's wire name, e.g. `"blockchain_module"`.
///
/// `Arc<str>` because the id is cloned onto every parsed log line and every
/// buffer/pane that names the module; it is persisted inside
/// `dashboard.json.gz` as a bare string.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ModuleId(Arc<str>);

impl ModuleId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The pseudo-module the daemon's own untagged lines are attributed to.
    pub fn daemon() -> Self {
        Self::from(DAEMON)
    }

    /// Sidebar label: `blockchain_module` reads badly in a UI, `Blockchain`
    /// does. The `_module` suffix is carried by every module we ship and adds
    /// nothing once the rows are grouped under a "Modules" heading.
    pub fn display_name(&self) -> String {
        let stem = self.0.strip_suffix("_module").unwrap_or(&self.0);

        stem.split('_')
            .filter(|word| !word.is_empty())
            .map(|word| {
                let mut chars = word.chars();

                match chars.next() {
                    Some(first) => {
                        first.to_uppercase().collect::<String>()
                            + chars.as_str()
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ModuleId {
    fn from(id: &str) -> Self {
        Self(Arc::from(id))
    }
}

impl From<String> for ModuleId {
    fn from(id: String) -> Self {
        Self(Arc::from(id))
    }
}

/// Load state.
///
/// `status --json` only ever reports `loaded` / `not_loaded`. Two of these
/// are therefore ours, and both exist because that two-word vocabulary
/// collapses states a person watching a module needs told apart:
///
/// - `Crashed`, because a module that aborts is reported as `not_loaded` on
///   the very next poll, which is indistinguishable from "never started" and
///   would silently render an idle row where a restart should be offered.
/// - `Loading`, because a module the app is bringing up — or one the backend
///   is rebuilding after a restart — also reads `not_loaded` until it
///   finishes, and "not loaded" reads as *off* where the truth is *not yet*.
///
/// Neither can be inferred from a poll alone, which is exactly why
/// [`Status::from_daemon`] cannot produce them: they are held by
/// [`crate::Session`], which is the only place that also sees the backend
/// phase and the log.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Loaded,
    /// A load we know is in flight, because we watched it start. Never
    /// guessed from a poll: a module an operator loaded behind our back is
    /// only ever seen already-`Loaded`, and claiming we saw it loading would
    /// be a lie told to look informative.
    Loading,
    NotLoaded,
    Crashed,
    /// A value the daemon reported that we do not recognise. Kept verbatim
    /// rather than dropped or panicked on, so a daemon that grows a new state
    /// degrades to a readable label instead of a lie.
    Unknown(String),
}

impl Status {
    /// Maps the daemon's `status --json` vocabulary. Anything else is kept as
    /// `Unknown` — this is a wire boundary, not a closed set.
    ///
    /// `Loading` is deliberately unreachable from here. Should a daemon ever
    /// report the word, `Unknown("loading")` renders the same label without
    /// letting a wire value masquerade as the client-side state the UI
    /// reasons about.
    pub fn from_daemon(status: &str) -> Self {
        match status {
            "loaded" => Self::Loaded,
            "not_loaded" => Self::NotLoaded,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn is_loaded(&self) -> bool {
        matches!(self, Self::Loaded)
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Loaded => "loaded",
            Self::Loading => "loading",
            Self::NotLoaded => "not loaded",
            Self::Crashed => "crashed",
            Self::Unknown(raw) => raw,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub id: ModuleId,
    pub status: Status,
    /// From the module manifest; absent until the daemon has been asked.
    pub version: Option<String>,
    /// As declared in the manifest, not as resolved by the daemon.
    pub dependencies: Vec<ModuleId>,
    /// Derived from `dependencies`, never configured: the module is in chat's
    /// dependency closure, so unloading it would take chat down with it and
    /// the UI must not offer the action. A later increment enforces this; the
    /// model only records it.
    pub protected: bool,
    /// Module events the backend counted in the poll window the last report
    /// closed — the "there is progress" pulse of `logos-modules.md` §2b, sampled
    /// rather than streamed because a syncing blockchain node emits far too
    /// many to render one by one. Zero means the module said nothing in
    /// that window, which is also what a module that never emits says.
    pub recent_events: u64,
}

impl Module {
    pub fn display_name(&self) -> String {
        self.id.display_name()
    }
}

/// One entry of the hardcoded module set.
struct Staged {
    id: &'static str,
    version: &'static str,
    dependencies: &'static [&'static str],
}

/// The staged module set, before the daemon has said anything about it.
///
/// Everything starts `NotLoaded`: that is what `status --json` reports for a
/// module that exists on disk and has not been loaded, so the pre-poll state
/// and the first poll agree unless the daemon says otherwise.
pub fn catalog() -> Vec<Module> {
    let mut protected = vec![CHAT];
    let mut cursor = 0;

    while cursor < protected.len() {
        let id = protected[cursor];
        cursor += 1;

        let Some(staged) = STAGED.iter().find(|staged| staged.id == id) else {
            continue;
        };

        for dependency in staged.dependencies {
            if !protected.contains(dependency) {
                protected.push(dependency);
            }
        }
    }

    STAGED
        .iter()
        .map(|staged| Module {
            id: ModuleId::from(staged.id),
            status: Status::NotLoaded,
            version: Some(staged.version.to_owned()),
            dependencies: staged
                .dependencies
                .iter()
                .map(|dependency| ModuleId::from(*dependency))
                .collect(),
            protected: protected.contains(&staged.id),
            recent_events: 0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_stages_the_four_shipped_modules() {
        let catalog = catalog();

        assert_eq!(
            catalog
                .iter()
                .map(|module| module.id.to_string())
                .collect::<Vec<_>>(),
            vec![
                "chat_module",
                "delivery_module",
                "capability_module",
                "blockchain_module",
            ],
        );

        assert!(
            catalog
                .iter()
                .all(|module| module.status == Status::NotLoaded)
        );
    }

    /// The guard is derived from the declared dependencies, so it must cover
    /// chat *and* the delivery module chat pulls in — and nothing else.
    #[test]
    fn chats_dependency_closure_is_protected() {
        let protected = catalog()
            .into_iter()
            .filter(|module| module.protected)
            .map(|module| module.id.to_string())
            .collect::<Vec<_>>();

        assert_eq!(protected, vec!["chat_module", "delivery_module"]);
    }

    #[test]
    fn display_names_drop_the_module_suffix() {
        assert_eq!(
            ModuleId::from("blockchain_module").display_name(),
            "Blockchain"
        );
        assert_eq!(
            ModuleId::from("delivery_module").display_name(),
            "Delivery"
        );
        assert_eq!(ModuleId::daemon().display_name(), "Logoscore");
        assert_eq!(ModuleId::from("").display_name(), "");
    }

    /// A state we do not know must survive round-tripping rather than
    /// collapsing into one of the states we do know.
    #[test]
    fn unrecognised_daemon_status_is_kept_verbatim() {
        let status = Status::from_daemon("reloading");

        assert_eq!(status, Status::Unknown("reloading".to_owned()));
        assert_eq!(status.label(), "reloading");
        assert!(!status.is_loaded());
    }

    /// `Loading` is a claim about something we watched happen, so no wire
    /// value may mint one. A daemon that grows the word still reads as
    /// "loading" on screen — through `Unknown`, which promises nothing.
    #[test]
    fn the_wire_cannot_mint_a_loading_status() {
        assert_eq!(
            Status::from_daemon("loading"),
            Status::Unknown("loading".to_owned()),
        );
        assert_eq!(Status::Loading.label(), "loading");
        assert!(!Status::Loading.is_loaded());
    }
}
