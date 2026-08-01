//! Class: the module monitor — a sidebar group and a read-only log pane
//! that live alongside conversations rather than inside them.
//!
//! One invariant carries most of the weight: a module pane is not a
//! conversation. Conversation identity is ephemeral, so the app sweeps dead
//! conversation panes on every snapshot and prunes persisted `convo:`
//! settings on every load — and a module id, coming from a hardcoded catalog,
//! is stable across both. Every case here is some form of "the conversation
//! machinery did not mistake a module for one of its own".
//!
//! The other half of the feature — a log that has no backlog to fetch and
//! must therefore be `Full` from birth — is its own class, in
//! [`super::logs`].

use crate::testkit::build::CRASH;
use crate::testkit::{App, build};

/// The motivating trap. `reconcile_conversations` empties every pane whose
/// conversation the backend no longer has, and conversation ids are
/// ephemeral across restarts — so it runs on every snapshot, including one
/// that clears the lot. A module pane names no conversation and must come
/// through untouched.
#[tokio::test(start_paused = true)]
async fn a_module_pane_survives_a_snapshot_that_clears_every_conversation() {
    let previous_run = build::focused_on_module(
        build::persisted(
            build::split_v(
                build::pane("dead-conversation"),
                build::module_pane("blockchain_module"),
            ),
            None,
        ),
        "blockchain_module",
    );

    let mut app = App::restore(previous_run);

    app.online(&[]);

    assert_eq!(
        app.panes(),
        vec!["empty".to_owned(), "module:blockchain_module".to_owned()],
        "\nlayout: {}\nfocus:  {}\n",
        app.layout(),
        app.focus(),
    );

    assert_eq!(
        app.focus(),
        "module:blockchain_module",
        "focus on a module pane is not focus on a conversation",
    );
}

/// The same trap on the other clock. Ids are minted per backend run, so a
/// restart mid-session invalidates every pane at once — and the snapshot that
/// says so arrives long after launch, when the module pane has a log worth
/// keeping. Reconciliation may empty neither the pane nor its history.
#[tokio::test(start_paused = true)]
async fn a_later_snapshot_that_clears_every_conversation_spares_the_module() {
    let previous_run = build::focused_on_module(
        build::persisted(
            build::split_v(
                build::pane("c1"),
                build::module_pane("blockchain_module"),
            ),
            None,
        ),
        "blockchain_module",
    );

    let mut app = App::restore(previous_run);

    app.online(&["c1"]);
    app.module_logs(&[
        "[2026-07-31 21:27:38.166] [out] [blockchain_module] \
         2026-08-01T03:27:38.166480Z  INFO logos_blockchain::storage: \
         Service 'Storage' is ready.",
        "[2026-07-31 21:27:39.010] [out] [blockchain_module] \
         2026-08-01T03:27:39.010200Z  INFO logos_blockchain::sync: \
         Synced to block 1158",
    ]);

    assert_eq!(app.layout(), "split-v(convo:c1, module:blockchain_module)");

    // The backend restarted: every id it had a moment ago is gone.
    app.backend(build::snapshot(&[]));

    assert_eq!(
        app.layout(),
        "split-v(empty, module:blockchain_module)",
        "\nfocus: {}\nlive:  {:?}\nscreen:\n{}\n",
        app.focus(),
        app.live_conversations(),
        app.screen(),
    );
    assert_eq!(
        app.focus(),
        "module:blockchain_module",
        "\nlayout: {}\nscreen:\n{}\n",
        app.layout(),
        app.screen(),
    );
    assert_eq!(app.stale_panes(), Vec::<String>::new());
    assert_eq!(
        app.module_lines("blockchain_module"),
        vec![
            "Service 'Storage' is ready.".to_owned(),
            "Synced to block 1158".to_owned(),
        ],
        "reconciliation threw away a log it cannot ask for again\n{}",
        app.screen(),
    );
}

/// Module identity is stable — the catalog is hardcoded — which is the whole
/// reason a module pane may be restored while a conversation pane may not.
/// The load path prunes persisted `convo:` settings unconditionally; a
/// `module:` key names the same module next launch and has to come back.
#[tokio::test(start_paused = true)]
async fn a_persisted_module_pane_and_its_settings_survive_the_load_sweep() {
    let previous_run = build::with_settings(
        build::with_settings(
            build::focused_on_module(
                build::persisted(
                    build::split_v(
                        build::pane("gone"),
                        build::module_pane("delivery_module"),
                    ),
                    None,
                ),
                "delivery_module",
            ),
            data::Buffer::Conversation("gone".into()),
        ),
        data::Buffer::Module("delivery_module".into()),
    );

    let mut app = App::restore(previous_run.clone());

    app.online(&["fresh"]);

    assert_eq!(app.layout(), "split-v(empty, module:delivery_module)");
    assert_eq!(app.focus(), "module:delivery_module");

    let controls = app.controls();

    assert!(
        !controls.iter().any(|control| control.contains("delivery")),
        "the backend was asked to load a module's messages: {controls:?}",
    );

    // `reload_config` is the one path that goes back through
    // `Dashboard::load`, which is where the prune sweep runs.
    app.reload_config(previous_run).await;

    let persisted = app.persisted();

    assert_eq!(app.layout(), "split-v(empty, module:delivery_module)");
    assert_eq!(
        app.focus(),
        "module:delivery_module",
        "\nscreen:\n{}\n",
        app.screen(),
    );
    assert!(
        persisted
            .buffer_settings
            .get(&data::Buffer::Module("delivery_module".into()))
            .is_some(),
        "a module's settings were swept with the conversations'",
    );
    assert!(
        persisted
            .buffer_settings
            .get(&data::Buffer::Conversation("gone".into()))
            .is_none(),
        "the conversation sweep stopped sweeping conversations",
    );
}

/// The `Partial` trap, from the UI side: nothing will ever call `load_full`
/// for a module, so a line handed to the app has to be readable immediately
/// and for the rest of the session.
#[tokio::test(start_paused = true)]
async fn log_lines_render_without_any_snapshot_ever_loading_them() {
    let mut app = App::restore(build::persisted(
        build::module_pane("blockchain_module"),
        None,
    ));

    app.online(&[]);

    app.module_logs(&["[2026-07-31 21:27:38.166] [out] [blockchain_module] \
         2026-08-01T03:27:38.166480Z  INFO logos_blockchain::storage: \
         Service 'Storage' is ready."]);

    assert_eq!(
        app.module_lines("blockchain_module"),
        vec!["Service 'Storage' is ready.".to_owned()]
    );
}

/// Volume is the whole problem: delivery emitted ~2000 lines in minutes and
/// 38% of them were `DBG`. The floor is applied before a message is ever
/// built, so a filtered line costs its parse and nothing else.
#[tokio::test(start_paused = true)]
async fn debug_output_is_filtered_out_by_default_and_info_is_kept() {
    let mut app = App::restore(build::persisted(
        build::module_pane("delivery_module"),
        None,
    ));

    app.online(&[]);

    app.module_logs(&[
        "[2026-07-31 21:32:59.363] [out] [delivery_module] DBG 2026-07-31 \
         21:32:59.363-06:00 mounting relay protocol",
        "[2026-07-31 21:32:59.364] [out] [delivery_module] INF 2026-07-31 \
         21:32:59.364-06:00 Node started",
        "[2026-07-31 21:32:59.365] [out] [delivery_module] ERR 2026-07-31 \
         21:32:59.365-06:00 failed to dial peer",
    ]);

    assert_eq!(
        app.module_lines("delivery_module"),
        vec!["Node started".to_owned(), "failed to dial peer".to_owned(),]
    );
}

/// Findings §5: a module aborts, and the daemon then reports it `not_loaded`
/// — the same word an idle module gets. The log is the only place the
/// difference exists, so a crash line has to move the row, and it has to do
/// so even though the line that carries it is enveloped as `info`.
#[tokio::test(start_paused = true)]
async fn a_crash_in_the_log_moves_the_row_out_of_idle() {
    let mut app = App::restore(build::persisted(
        build::module_pane("blockchain_module"),
        None,
    ));

    app.online(&[]);
    app.backend(build::modules(&[("blockchain_module", "loaded")]));

    assert_eq!(app.module_status("blockchain_module"), "loaded");

    app.module_logs(CRASH);

    assert_eq!(app.module_status("blockchain_module"), "crashed");

    // The next poll calls it merely not loaded, which is what a module that
    // was never started reports too. The crash has to outlive it.
    app.backend(build::modules(&[("blockchain_module", "not_loaded")]));

    assert_eq!(app.module_status("blockchain_module"), "crashed");
    assert!(
        app.shows("crashed"),
        "a crashed module must say so on screen, not render as idle\n{}",
        app.screen(),
    );
}

/// The rule: no header, no placeholder, no spacer — a run before the first
/// `listModules` report has to look exactly like one built before the
/// monitor existed. The row list is empty, and the sidebar's group loop
/// already drops empty groups.
#[tokio::test(start_paused = true)]
async fn the_sidebar_shows_nothing_at_all_until_the_daemon_reports() {
    let mut app = App::fresh();

    app.online(&[]);

    let before = app.text();

    assert!(
        !app.shows("Blockchain") && !app.shows("Delivery"),
        "a module row appeared before any module was reported\n{}",
        app.screen(),
    );

    app.backend(build::modules(&[
        ("delivery_module", "loaded"),
        ("blockchain_module", "not_loaded"),
    ]));

    assert!(app.shows("Delivery"), "{}", app.screen());
    assert!(app.shows("Blockchain"), "{}", app.screen());
    assert_ne!(before, app.text());
}

/// The pulse is the answer to "is it stuck?", which is the question a
/// `Bootstrapping` module raises and cannot answer itself: `logos-modules.md` §2b
/// clocked a cold start at ~23 minutes, during which the only honest signal
/// that work is happening is that events keep arriving. The backend samples
/// them per poll instead of forwarding 1158 of them, and the row shows the
/// sample.
///
/// The zero is as load-bearing as the count. A module that stops emitting
/// has to stop reading as busy, so the last busy window is followed by a
/// report of zero — and a row that never emits anything shows nothing at
/// all rather than a permanent `0`.
#[tokio::test(start_paused = true)]
async fn a_module_row_shows_the_event_pulse_the_backend_sampled() {
    let mut app = App::fresh();

    app.online(&[]);
    app.backend(build::modules_pulsing(&[
        ("blockchain_module", "loaded", 384),
        ("delivery_module", "loaded", 0),
    ]));

    assert_eq!(
        app.module_pulse("Blockchain"),
        "384",
        "a syncing module reads as idle\n{}",
        app.screen(),
    );
    assert_eq!(
        app.module_pulse("Delivery"),
        "",
        "a module that emits nothing must not carry a number\n{}",
        app.screen(),
    );

    // Caught up: one block every few seconds instead of a flood.
    app.backend(build::modules_pulsing(&[
        ("blockchain_module", "loaded", 2),
        ("delivery_module", "loaded", 0),
    ]));
    assert_eq!(app.module_pulse("Blockchain"), "2", "{}", app.screen());

    // Stopped. The row has to say so.
    app.backend(build::modules_pulsing(&[
        ("blockchain_module", "loaded", 0),
        ("delivery_module", "loaded", 0),
    ]));
    assert_eq!(
        app.module_pulse("Blockchain"),
        "",
        "a module that went quiet still reads as busy\n{}",
        app.screen(),
    );
}

/// Modules open through the same pane machinery conversations use, and the
/// pane persists under its own key namespace — `module:`, deliberately not
/// `convo:`, which is the prefix `Dashboard::load` sweeps.
#[tokio::test(start_paused = true)]
async fn opening_a_module_from_the_sidebar_opens_a_persistable_pane() {
    let mut app = App::fresh();

    app.online(&["c1"]);
    app.backend(build::modules(&[("delivery_module", "loaded")]));

    app.open_module("delivery_module");

    assert!(
        app.panes().contains(&"module:delivery_module".to_owned()),
        "panes: {:?}",
        app.panes(),
    );

    // Opening it twice focuses the pane that is already there rather than
    // splitting a second one off.
    app.open_module("delivery_module");

    assert_eq!(
        app.panes()
            .iter()
            .filter(|key| *key == "module:delivery_module")
            .count(),
        1,
        "panes: {:?}",
        app.panes(),
    );
}

/// A module pane with nothing in it yet says which of the four "nothing"
/// worlds it is in, rather than rendering an idle-looking blank.
#[tokio::test(start_paused = true)]
async fn an_empty_pane_says_why_it_is_empty() {
    let mut app = App::restore(build::without_sidebar(build::persisted(
        build::module_pane("blockchain_module"),
        None,
    )));

    app.online(&[]);

    assert!(
        app.shows("Waiting for the daemon"),
        "no module report has landed yet\n{}",
        app.screen(),
    );

    app.backend(build::modules(&[("blockchain_module", "not_loaded")]));

    assert!(
        app.shows("Blockchain is not loaded"),
        "an idle module is not the same as a silent one\n{}",
        app.screen(),
    );

    app.backend(build::modules(&[("blockchain_module", "loaded")]));

    assert!(app.shows("No log output yet"), "{}", app.screen());
}

/// The sidebar is where a module is watched from, and the one distinction it
/// must never fudge is dead versus never started: `listModules` calls both
/// `not_loaded`, so a row that only dims for a crash would say a module the
/// user is relying on is merely idle. The crashed row breaks the indicator's
/// shape, not just its colour, which is the half that survives being read as
/// text — the rest is checked against the status the icon is drawn from.
#[tokio::test(start_paused = true)]
async fn the_sidebar_tells_a_crashed_module_from_an_idle_one() {
    let mut app = App::fresh();

    app.online(&[]);
    app.backend(build::modules(&[
        ("blockchain_module", "not_loaded"),
        ("delivery_module", "loaded"),
    ]));

    assert_eq!(
        app.alarmed_modules(),
        Vec::<String>::new(),
        "nothing has crashed yet\n{}",
        app.screen(),
    );
    assert_eq!(app.module_status("blockchain_module"), "not loaded");
    assert_eq!(app.module_status("delivery_module"), "loaded");

    app.module_logs(CRASH);

    assert_eq!(
        app.alarmed_modules(),
        vec!["Blockchain".to_owned()],
        "\nstatus: {}\nscreen:\n{}\n",
        app.module_status("blockchain_module"),
        app.screen(),
    );
    assert_eq!(app.module_status("blockchain_module"), "crashed");
    assert_eq!(
        app.module_status("delivery_module"),
        "loaded",
        "one module's crash moved another module's row",
    );

    // The poll that follows: the daemon calls the dead module and the idle
    // one by the same word. Only the log knows the difference.
    app.backend(build::modules(&[
        ("blockchain_module", "not_loaded"),
        ("delivery_module", "not_loaded"),
    ]));

    assert_eq!(
        app.alarmed_modules(),
        vec!["Blockchain".to_owned()],
        "a crash was forgotten the moment the daemon called it not_loaded\n{}",
        app.screen(),
    );
    assert_eq!(app.module_status("delivery_module"), "not loaded");
}

/// Read-only is structural, not a disabled widget: the composer is built in
/// exactly one place — a conversation pane — and a module pane has nothing
/// for one to talk to. So the count of things on screen that can be typed
/// into goes to zero when the conversation is replaced by a module, and the
/// pane never says a word to the backend no matter what it is fed.
#[tokio::test(start_paused = true)]
async fn a_module_pane_has_no_composer_and_nothing_to_say_to_the_backend() {
    let mut app = App::restore(build::persisted(build::pane("c1"), Some("c1")));

    app.online(&["c1"]);
    app.backend(build::modules(&[("blockchain_module", "loaded")]));

    assert_eq!(
        app.composers(),
        1,
        "a conversation pane is what a composer belongs to\n{}",
        app.screen(),
    );

    // Everything the conversation asked for on the way in is its business.
    let _ = app.controls();

    app.replace_pane(data::Buffer::Module("blockchain_module".into()));

    assert_eq!(app.panes(), vec!["module:blockchain_module".to_owned()]);
    assert_eq!(
        app.composers(),
        0,
        "\nlayout: {}\nfocus:  {}\nscreen:\n{}\n",
        app.layout(),
        app.focus(),
        app.screen(),
    );

    app.module_logs(CRASH);
    app.open_module("blockchain_module");

    assert_eq!(
        app.composers(),
        0,
        "a crashed module grew somewhere to type\n{}",
        app.screen(),
    );
    assert_eq!(
        app.controls(),
        Vec::<String>::new(),
        "a read-only pane asked the backend for something",
    );
}

/// Attribution is resolved once, in the parser, and the pane a line lands on
/// follows from it. Two modules logging into the same file at the same time
/// is the normal case, not the exotic one — and the daemon's own lines
/// belong to neither of them.
#[tokio::test(start_paused = true)]
async fn interleaved_lines_land_on_the_module_that_emitted_them() {
    let mut app = App::restore(build::persisted(
        build::split_v(
            build::module_pane("blockchain_module"),
            build::module_pane("delivery_module"),
        ),
        None,
    ));

    app.online(&[]);
    app.module_logs(&[
        "[2026-07-31 21:32:59.364] [out] [delivery_module] INF 2026-07-31 \
         21:32:59.364-06:00 Node started",
        "[2026-07-31 21:27:38.166] [out] [blockchain_module] \
         2026-08-01T03:27:38.166480Z  INFO logos_blockchain::storage: \
         Service 'Storage' is ready.",
        "[2026-07-31 21:27:28.193] [info] [logos] Module loaded: \
         blockchain_module",
        "[2026-07-31 21:32:59.401] [out] [delivery_module] INF 2026-07-31 \
         21:32:59.401-06:00 Listening on /ip4/127.0.0.1/tcp/60000",
        "[2026-07-31 21:27:39.010] [out] [blockchain_module] \
         2026-08-01T03:27:39.010200Z  INFO logos_blockchain::sync: \
         Synced to block 1158",
    ]);

    assert_eq!(
        app.module_lines("blockchain_module"),
        vec![
            "Service 'Storage' is ready.".to_owned(),
            "Synced to block 1158".to_owned(),
        ],
        "\nscreen:\n{}\n",
        app.screen(),
    );
    assert_eq!(
        app.module_lines("delivery_module"),
        vec![
            "Node started".to_owned(),
            "Listening on /ip4/127.0.0.1/tcp/60000".to_owned(),
        ],
        "\nscreen:\n{}\n",
        app.screen(),
    );

    // The daemon's own line names a module in its text without being that
    // module's output. It carries no module tag, so it is filed under the
    // daemon's own id — neither pane above may claim it, and it may not be
    // dropped either.
    assert_eq!(
        app.module_lines(data::module::DAEMON),
        vec!["Module loaded: blockchain_module".to_owned()],
    );
}
