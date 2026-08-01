use std::hash::Hash;
use std::path::PathBuf;

pub use data::stream::{self, *};
use data::{Config, environment};
use futures::Stream;
use iced::Subscription;

use crate::mock;

/// The single backend subscription. One session per app run: the key is a
/// constant, so iced never restarts the stream across config reloads.
pub fn subscription(
    config: &Config,
    guard: ipc::Acquired,
) -> Subscription<stream::Update> {
    struct State {
        backend_config: BackendConfig,
        mock: bool,
    }

    impl State {
        fn run(&self) -> impl Stream<Item = stream::Update> + use<> {
            let driver = if self.mock {
                Driver::Fake(mock::driver())
            } else {
                Driver::Live
            };

            stream::run(self.backend_config.clone(), driver)
        }
    }

    impl PartialEq for State {
        fn eq(&self, other: &Self) -> bool {
            self.mock.eq(&other.mock)
        }
    }

    impl Hash for State {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            "logos-backend".hash(state);
            self.mock.hash(state);
        }
    }

    Subscription::run_with(
        State {
            backend_config: backend_config(config, guard),
            mock: config.logos.mock,
        },
        State::run,
    )
}

/// The combined daemon log, for readers that want it without owning a
/// session. Routed through [`BackendConfig`] rather than assembled here so
/// the tailer and the daemon the session spawns can never point at different
/// files — the whole path is one derivation from `state_dir`.
pub fn daemon_log_path(config: &Config) -> PathBuf {
    BackendConfig::new(state_dir(config)).daemon_log_path()
}

fn state_dir(config: &Config) -> PathBuf {
    config
        .logos
        .instance_dir
        .clone()
        .unwrap_or_else(environment::logos_instance_dir)
}

fn backend_config(config: &Config, guard: ipc::Acquired) -> BackendConfig {
    let logos = &config.logos;

    let mut backend_config = BackendConfig::new(state_dir(config));
    // The module set is ours, hardcoded and staged by us — the session is
    // told what to track rather than discovering it, so `listModules` stays
    // a status feed and never becomes a plugin surface.
    backend_config.modules = data::module::catalog()
        .into_iter()
        .map(|module| module.id.to_string())
        .collect();
    backend_config.delivery_preset = logos.delivery_preset.clone();
    backend_config.installation_name = logos.installation_name.clone();
    backend_config.use_wildcard_watch = logos.use_wildcard_watch;
    backend_config.artifacts.logoscore_bin = logos.daemon_path.clone();
    backend_config.artifacts.modules_dir = logos.modules_dir.clone();
    // Only the instance that owns the state dir may clear the daemon it
    // finds there; without the guard that daemon could be a live sibling's.
    backend_config.reap_stale_daemon = guard == ipc::Acquired::Owner;

    backend_config
}
