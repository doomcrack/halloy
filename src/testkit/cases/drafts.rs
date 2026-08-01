//! Class: drafts and unread. Text the user typed must survive a refused
//! send, and a conversation with no pane open must still accrue unread.

use crate::testkit::{App, build};

/// A refused send hands the text back and says why, so nothing the user
/// typed is lost to a busy module.
#[tokio::test(start_paused = true)]
async fn a_refused_send_returns_the_draft_and_reports_why() {
    let mut app = App::restore(build::persisted(build::pane("c1"), Some("c1")));

    app.online(&["c1"]);

    assert_eq!(app.draft("c1"), "");

    app.backend(build::send_failed("c1", "hey there", "module busy"));

    assert_eq!(app.draft("c1"), "hey there");
    assert!(
        app.shows("Failed to send: module busy"),
        "the failure never reached the status strip:\n{}",
        app.screen(),
    );
}

/// Unread accrues for a conversation with no pane open — `sync_histories`
/// opens a history per known conversation exactly so this works.
#[tokio::test(start_paused = true)]
async fn unread_accrues_for_a_conversation_with_no_pane_open() {
    let mut app = App::fresh();

    app.online(&["c1", "c2"]);

    assert_eq!(app.panes(), vec!["empty".to_owned()]);
    assert_eq!(app.unread("c1"), 0);

    app.backend(build::received("c1", "first"));
    app.backend(build::received("c1", "second"));
    app.backend(build::received("c2", "elsewhere"));

    assert_eq!(app.unread("c1"), 2);
    assert_eq!(app.unread("c2"), 1);
}
