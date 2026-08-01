//! The module monitor's log feed, as an iced subscription.
//!
//! Everything interesting is in [`data::module::tail`]; this is the adapter
//! that gives it a subscription identity. Deliberately not part of the
//! backend session subscription: tailing a file needs no lp client, no daemon
//! handshake and no token, and keeping it separate means the log pane keeps
//! filling while the session is down — which is exactly when its contents
//! matter.

use std::hash::Hash;
use std::path::PathBuf;

use data::Config;
use data::module::tail::{self, Line};
use iced::Subscription;

use crate::stream;

/// Follows the daemon's combined log, emitting one message per
/// [`tail::POLL_INTERVAL`] in which anything was written.
///
/// Keyed on a constant, exactly as [`stream::subscription`] is: one tail per
/// app run, following the path that was resolved at launch. The daemon is
/// spawned once, into the instance dir current at launch, and the supervisor
/// holds that write handle for the whole run — so keying this on the path
/// would let a config reload point the tail at a file no daemon writes while
/// the live daemon kept appending to the one nobody reads, and the pane would
/// go silently and permanently empty.
///
/// Whichever increment makes a config reload restart the backend must restart
/// this with it; the two keys have to change together or not at all.
pub fn subscription(config: &Config) -> Subscription<Vec<Line>> {
    struct Tail(PathBuf);

    impl Tail {
        fn run(&self) -> impl futures::Stream<Item = Vec<Line>> + use<> {
            tail::follow(self.0.clone())
        }
    }

    impl Hash for Tail {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            "module-log".hash(state);
        }
    }

    Subscription::run_with(Tail(stream::daemon_log_path(config)), Tail::run)
}
