//! Class: offline/disabled states. What the UI says while delivery is not
//! online, and that it stops saying it the moment delivery recovers.

use crate::testkit::{App, build};

#[tokio::test(start_paused = true)]
async fn offline_empty_state_and_status_strip_track_delivery() {
    let mut app = App::fresh();

    app.connect();

    assert!(
        app.shows("Waiting for connection..."),
        "cold start should not claim there are no conversations yet:\n{}",
        app.screen(),
    );

    app.script(build::startup());
    app.backend(build::snapshot(&[]));

    assert!(
        app.shows("No conversations yet"),
        "online with an empty snapshot should say so:\n{}",
        app.screen(),
    );
    assert!(!app.shows("Waiting for connection..."));
    assert!(!app.shows("Connecting..."));

    app.backend(build::offline("daemon exited"));

    assert!(app.shows("Waiting for connection..."));
    assert!(
        app.shows("Delivery error: daemon exited"),
        "the status strip should spell out why actions are gated:\n{}",
        app.screen(),
    );
}
