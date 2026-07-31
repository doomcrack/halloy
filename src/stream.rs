use std::hash::Hash;

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

fn backend_config(config: &Config, guard: ipc::Acquired) -> BackendConfig {
    let logos = &config.logos;

    let mut backend_config = BackendConfig::new(
        logos
            .instance_dir
            .clone()
            .unwrap_or_else(environment::logos_instance_dir),
    );
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
