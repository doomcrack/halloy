//! Class: async ordering and the load windows. A restored pane asks the
//! backend for its messages before a controller exists, so the request is
//! dropped and has to be re-issued; and an empty thread must not be called
//! empty before its rows have had time to land.

use std::time::Duration;

use crate::testkit::{App, build};

/// `Dashboard::restore` runs before the stream hands over its controller,
/// so its `LoadMessages` goes nowhere. The snapshot has to re-issue it, or
/// a restored pane stays blank forever.
#[tokio::test(start_paused = true)]
async fn restored_pane_reissues_its_message_load_after_the_snapshot() {
    let mut app = App::restore(build::persisted(build::pane("c1"), Some("c1")));

    assert_eq!(
        app.controls(),
        Vec::<String>::new(),
        "restore cannot have reached a backend that does not exist yet",
    );

    app.connect();
    app.script(build::startup());

    assert_eq!(
        app.controls(),
        Vec::<String>::new(),
        "nothing is known about conversations before the first snapshot",
    );

    app.backend(build::snapshot(&["c1"]));

    assert!(
        app.controls().contains(&"LoadMessages(c1)".to_owned()),
        "the snapshot must re-issue the load the restore dropped",
    );

    app.backend(build::messages_loaded("c1", &["hello", "again"]));

    assert_eq!(app.messages("c1"), vec!["hello", "again"]);
}

/// No flashing empty state: a thread whose messages have not landed shows
/// nothing during the grace window, and only calls itself empty once the
/// settle window after the load has passed.
#[tokio::test(start_paused = true)]
async fn empty_thread_waits_out_its_grace_and_settle_windows() {
    let mut app = App::restore(build::without_sidebar(build::persisted(
        build::pane("c1"),
        Some("c1"),
    )));

    app.online(&["c1"]);

    assert!(
        !app.shows("No messages yet"),
        "empty state flashed before the module answered:\n{}",
        app.screen(),
    );

    app.advance(Duration::from_millis(200)).await;
    app.backend(build::messages_loaded("c1", &[]));

    assert!(
        !app.shows("No messages yet"),
        "empty state flashed inside the settle window:\n{}",
        app.screen(),
    );

    app.advance(Duration::from_millis(400)).await;

    assert!(
        app.shows("No messages yet"),
        "settled thread never said it was empty:\n{}",
        app.screen(),
    );
}
