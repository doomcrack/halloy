//! Class: the buffers that have no backlog to fetch — the app's own log and
//! every module's.
//!
//! A conversation is born `Partial` and promoted by the snapshot the backend
//! sends back. Nothing sends one for a log: the lines are pushed in as they
//! are tailed, and no code path will ever call `load_full` for one. A log
//! born `Partial` therefore parks every line it is handed in
//! `pending_messages` and renders empty for the rest of the session — which
//! is exactly what the `Logs` buffer did before this change.
//!
//! These cases pin the fix from the outside: a line is on the pane the
//! moment it arrives, and it is still there after the pane that showed it is
//! gone — `Manager::track` demotes every resource it stops tracking, and a
//! log demoted is a log lost.
//!
//! A log *row* leaves no text node behind (it is built from
//! `selectable_text`), so what the pane holds is read with
//! [`App::module_lines`] / [`App::log_lines`] — the same view the pane draws
//! from, and empty for a history still `Partial`. The screen is asked the
//! one question it can answer: whether the pane fell back to the "nothing
//! here yet" placeholder it would still be showing if the history were.

use data::config::logs::LevelFilter;
use data::log::Level;

use crate::testkit::build::CRASH;
use crate::testkit::{App, build};

/// The pre-existing bug, stated as a test: the app's own log pane rendered
/// empty because nothing promotes `Kind::Logs`. Same class as a module log,
/// same one-line fix, so it is pinned in the same place.
#[tokio::test(start_paused = true)]
async fn the_app_log_pane_renders_records_nothing_will_ever_load() {
    let mut app = App::fresh();

    app.online(&[]);
    app.open_logs();
    app.app_logs(&[
        (Level::Info, "connected to the daemon"),
        (Level::Debug, "internal bookkeeping nobody asked for"),
        (Level::Error, "delivery went away"),
    ]);

    assert_eq!(
        app.log_lines(),
        vec![
            "connected to the daemon".to_owned(),
            "delivery went away".to_owned(),
        ],
        "\nscreen:\n{}\n",
        app.screen(),
    );
    assert_eq!(
        app.controls(),
        Vec::<String>::new(),
        "the app's own log asked the backend for a backlog",
    );
}

/// The other half of the same fix. `Manager::track` runs on every pane
/// change and demotes whatever is no longer open — harmless for a
/// conversation, which refetches, and fatal for a log, which cannot. Closing
/// a module pane and opening it again must show the same lines.
#[tokio::test(start_paused = true)]
async fn a_module_log_outlives_the_pane_that_was_showing_it() {
    let mut app = App::fresh();

    app.online(&["c1"]);
    app.backend(build::modules(&[("delivery_module", "loaded")]));
    app.open_module("delivery_module");
    app.module_logs(&[
        "[2026-07-31 21:32:59.364] [out] [delivery_module] INF 2026-07-31 \
         21:32:59.364-06:00 Node started",
        "[2026-07-31 21:32:59.401] [out] [delivery_module] ERR 2026-07-31 \
         21:32:59.401-06:00 failed to dial peer",
    ]);

    let logged =
        vec!["Node started".to_owned(), "failed to dial peer".to_owned()];

    assert_eq!(app.module_lines("delivery_module"), logged);

    // What the screen can say about a `Partial` history: it renders as
    // empty, so the pane would still be showing its "nothing here yet"
    // placeholder over lines it is already holding.
    assert!(
        !app.shows("No log output yet"),
        "the pane called itself empty while holding two lines\n{}",
        app.screen(),
    );

    app.replace_pane(data::Buffer::Conversation("c1".into()));

    assert!(
        !app.panes().contains(&"module:delivery_module".to_owned()),
        "the pane did not close: {:?}",
        app.panes(),
    );

    let _ = app.controls();

    app.open_module("delivery_module");

    assert_eq!(
        app.module_lines("delivery_module"),
        logged,
        "closing the pane threw away a log nothing can fetch again\n{}",
        app.screen(),
    );
    assert!(
        !app.shows("No log output yet"),
        "the reopened pane rendered as empty\n{}",
        app.screen(),
    );

    let controls = app.controls();

    assert!(
        !controls.iter().any(|control| control.contains("delivery")),
        "reopening a module pane went looking for a backlog: {controls:?}",
    );
}

/// The floor is a comparison on an `Ord` level, so raising it drops
/// everything below it and nothing above. `Warn` is the interesting setting:
/// it has to keep the two levels above it and drop the two below, which a
/// filter written the wrong way round would get exactly backwards.
#[tokio::test(start_paused = true)]
async fn raising_the_floor_drops_everything_below_it_and_nothing_above() {
    let mut config = data::Config::default();

    config.modules.log_level = LevelFilter::Warn;

    let mut app = App::with_config(
        build::persisted(build::module_pane("delivery_module"), None),
        config,
    );

    app.online(&[]);
    app.module_logs(&[
        "[2026-07-31 21:32:59.360] [out] [delivery_module] DBG 2026-07-31 \
         21:32:59.360-06:00 mounting relay protocol",
        "[2026-07-31 21:32:59.364] [out] [delivery_module] INF 2026-07-31 \
         21:32:59.364-06:00 Node started",
        "[2026-07-31 21:32:59.370] [out] [delivery_module] WRN 2026-07-31 \
         21:32:59.370-06:00 relay shutdown failed",
        "[2026-07-31 21:32:59.401] [out] [delivery_module] ERR 2026-07-31 \
         21:32:59.401-06:00 failed to dial peer",
    ]);

    assert_eq!(
        app.module_lines("delivery_module"),
        vec![
            "relay shutdown failed".to_owned(),
            "failed to dial peer".to_owned(),
        ],
        "\nscreen:\n{}\n",
        app.screen(),
    );
}

/// A display setting may not decide what the app knows. `Off` admits no line
/// at all, and the crash is still learned — the check runs ahead of the
/// filter precisely because the abort in `logos-modules.md` §5 is announced under
/// envelope `[info]`, which any floor above it would have swallowed.
#[tokio::test(start_paused = true)]
async fn no_floor_can_hide_a_crash_from_the_row() {
    let mut config = data::Config::default();

    config.modules.log_level = LevelFilter::Off;

    let mut app = App::with_config(
        build::persisted(build::module_pane("blockchain_module"), None),
        config,
    );

    app.online(&[]);
    app.backend(build::modules(&[("blockchain_module", "loaded")]));
    app.module_logs(CRASH);

    assert_eq!(
        app.module_lines("blockchain_module"),
        Vec::<String>::new(),
        "`off` still rendered a line\n{}",
        app.screen(),
    );
    assert_eq!(
        app.module_status("blockchain_module"),
        "crashed",
        "the row believed a display setting over the log\n{}",
        app.screen(),
    );
    assert_eq!(
        app.alarmed_modules(),
        vec!["Blockchain".to_owned()],
        "\nscreen:\n{}\n",
        app.screen(),
    );
}
