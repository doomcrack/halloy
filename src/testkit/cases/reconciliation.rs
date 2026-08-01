//! Class: reconciling persisted layout/focus against the backend's
//! conversation snapshot. Conversation identity is ephemeral upstream, so
//! every launch restores panes whose ids may simply no longer exist.

use crate::testkit::{App, build};

/// A previous run's panes must not outlive their conversations: after the
/// first `ConversationsSnapshot`, nothing on screen may name an id the
/// backend does not have, and focus may not sit on one.
#[tokio::test(start_paused = true)]
async fn restored_panes_do_not_outlive_their_conversations() {
    let previous_run = build::persisted(
        build::split_v(build::pane("old-1"), build::pane("old-2")),
        Some("old-2"),
    );

    let mut app = App::restore(previous_run);

    app.online(&["new-1"]);

    assert_eq!(
        app.stale_panes(),
        Vec::<String>::new(),
        "\nlayout: {}\nfocus:  {}\nlive:   {:?}\nscreen:\n{}\n",
        app.layout(),
        app.focus(),
        app.live_conversations(),
        app.screen(),
    );

    assert_eq!(
        app.focus(),
        "none",
        "focus left pointing at a conversation the backend does not have",
    );
}

/// Conversations the snapshot still has keep their pane, so reconciliation
/// cannot be "throw everything away".
#[tokio::test(start_paused = true)]
async fn surviving_conversations_keep_their_pane() {
    let previous_run = build::persisted(build::pane("kept"), Some("kept"));

    let mut app = App::restore(previous_run);

    app.online(&["kept", "other"]);

    assert_eq!(app.panes(), vec!["convo:kept".to_owned()]);
    assert_eq!(app.focus(), "convo:kept");
    assert_eq!(app.stale_panes(), Vec::<String>::new());
}

/// A popped-out window is persisted too, and its pane is just as dead on
/// the next launch. Reconciliation walks every window, not the main tree:
/// here the only stale pane is the popout.
#[tokio::test(start_paused = true)]
async fn a_stale_popout_is_reconciled_with_the_main_window() {
    let previous_run = build::persisted(build::pane("kept"), Some("kept"));

    let mut app = App::restore(previous_run);

    app.popout("gone");

    app.online(&["kept"]);

    assert_eq!(
        app.stale_panes(),
        Vec::<String>::new(),
        "\npanes: {:?}\nlive:  {:?}\n",
        app.panes(),
        app.live_conversations(),
    );
    assert_eq!(
        app.panes(),
        vec!["convo:kept".to_owned(), "empty".to_owned()]
    );
}

/// Ids are minted per backend run, so a restart mid-session invalidates
/// panes that were live a moment ago. Every snapshot is authoritative, not
/// just the first one after launch.
#[tokio::test(start_paused = true)]
async fn a_later_snapshot_reconciles_a_conversation_that_went_away() {
    let previous_run = build::persisted(
        build::split_v(build::pane("c1"), build::pane("c2")),
        Some("c2"),
    );

    let mut app = App::restore(previous_run);

    app.online(&["c1", "c2"]);

    assert_eq!(app.layout(), "split-v(convo:c1, convo:c2)");
    assert_eq!(app.focus(), "convo:c2");

    // The resync after a restart: `c2` did not survive it.
    app.backend(build::snapshot(&["c1"]));

    assert_eq!(
        app.stale_panes(),
        Vec::<String>::new(),
        "\nlayout: {}\nfocus:  {}\nlive:   {:?}\n",
        app.layout(),
        app.focus(),
        app.live_conversations(),
    );
    assert_eq!(app.layout(), "split-v(convo:c1, empty)");
    assert_eq!(app.focus(), "none");
}

/// The point of reconciling before the histories are tracked: a dead id is
/// never asked about. Nothing the UI says to the backend may name one.
#[tokio::test(start_paused = true)]
async fn no_messages_are_requested_for_a_dead_conversation() {
    let previous_run = build::persisted(
        build::split_v(build::pane("old-1"), build::pane("new-1")),
        Some("old-1"),
    );

    let mut app = App::restore(previous_run);

    app.online(&["new-1"]);

    let controls = app.controls();

    assert!(
        !controls.iter().any(|control| control.contains("old-1")),
        "the backend was asked about a conversation it does not have: {controls:?}",
    );
    assert!(
        controls.contains(&"LoadMessages(new-1)".to_owned()),
        "the surviving pane never loaded its messages: {controls:?}",
    );
}

/// The other path that restores panes from disk: a config reload rebuilds
/// the screen through `load_from_state`, which rereads the same
/// `dashboard.json.gz` — dead ids included — and carries the live session
/// across. No snapshot follows it, so the invariant only holds if the
/// rebuild reconciles on its own.
#[tokio::test(start_paused = true)]
async fn a_config_reload_reconciles_the_screen_it_rebuilds() {
    let previous_run = build::persisted(
        build::split_v(build::pane("gone"), build::pane("kept")),
        Some("gone"),
    );

    let mut app = App::restore(previous_run.clone());

    app.online(&["kept"]);

    // Everything asked before the reload is the launch path's business.
    let _ = app.controls();

    app.reload_config(previous_run).await;

    assert_eq!(
        app.stale_panes(),
        Vec::<String>::new(),
        "\nlayout: {}\nfocus:  {}\nlive:   {:?}\nscreen:\n{}\n",
        app.layout(),
        app.focus(),
        app.live_conversations(),
        app.screen(),
    );
    assert_eq!(app.layout(), "split-v(empty, convo:kept)");
    assert_eq!(app.focus(), "none");

    let controls = app.controls();

    assert!(
        !controls.iter().any(|control| control.contains("gone")),
        "the rebuilt screen asked the backend about a conversation it does \
         not have: {controls:?}",
    );
}

/// Reconciliation is per pane, not per window: the survivor keeps focus
/// while its stale neighbour is emptied out from under it.
#[tokio::test(start_paused = true)]
async fn a_surviving_focused_pane_keeps_focus() {
    let previous_run = build::persisted(
        build::split_v(build::pane("gone"), build::pane("kept")),
        Some("kept"),
    );

    let mut app = App::restore(previous_run);

    app.online(&["kept"]);

    assert_eq!(app.layout(), "split-v(empty, convo:kept)");
    assert_eq!(app.focus(), "convo:kept");
    assert_eq!(app.stale_panes(), Vec::<String>::new());
}
