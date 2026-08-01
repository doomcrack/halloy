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
        ("capability_module", "not_loaded"),
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

    // The poll that follows: the daemon calls the dead module and the one
    // that was never started by the same word. Only what we already knew
    // tells them apart.
    app.backend(build::modules(&[
        ("blockchain_module", "not_loaded"),
        ("delivery_module", "loaded"),
        ("capability_module", "not_loaded"),
    ]));

    assert_eq!(
        app.alarmed_modules(),
        vec!["Blockchain".to_owned()],
        "a crash was forgotten the moment the daemon called it not_loaded\n{}",
        app.screen(),
    );
    assert_eq!(app.module_status("capability_module"), "not loaded");
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

/// `listModules` has two words and the row has to say more than two things.
/// The app loads chat itself, so while the ladder is at that step chat and
/// the dependency the daemon auto-loads with it are genuinely in flight —
/// and reporting them `not_loaded`, which is what the wire says, would read
/// as *off* rather than *not yet*.
#[tokio::test(start_paused = true)]
async fn the_module_the_app_is_loading_says_so_instead_of_reading_off() {
    let mut app = App::fresh();

    app.online(&[]);
    app.backend(build::modules(&[
        ("chat_module", "loaded"),
        ("delivery_module", "loaded"),
        ("blockchain_module", "not_loaded"),
    ]));

    app.script(build::loading_module());
    app.backend(build::modules(&[
        ("chat_module", "not_loaded"),
        ("delivery_module", "not_loaded"),
        ("blockchain_module", "not_loaded"),
    ]));

    assert_eq!(app.module_status("chat_module"), "loading");
    assert_eq!(app.module_status("delivery_module"), "loading");
    assert_eq!(
        app.module_status("blockchain_module"),
        "not loaded",
        "a module nothing is loading was claimed to be loading\n{}",
        app.screen(),
    );
    assert_eq!(
        app.alarmed_modules(),
        Vec::<String>::new(),
        "a restart in progress is not a sidebar full of crashes\n{}",
        app.screen(),
    );

    app.backend(build::modules(&[
        ("chat_module", "loaded"),
        ("delivery_module", "loaded"),
        ("blockchain_module", "not_loaded"),
    ]));

    assert_eq!(app.module_status("chat_module"), "loaded");
}

/// The lie the state must not tell. A module an operator loaded by hand is
/// something we only ever hear about afterwards, in a poll — so it goes from
/// idle straight to running, and is never dressed up as a load we watched.
#[tokio::test(start_paused = true)]
async fn a_module_loaded_outside_the_app_is_never_shown_loading() {
    let mut app = App::fresh();

    app.online(&[]);
    app.backend(build::modules(&[("blockchain_module", "not_loaded")]));

    assert_eq!(app.module_status("blockchain_module"), "not loaded");

    // `logoscore load-module blockchain_module`, from a terminal we know
    // nothing about. The next poll is the whole of our evidence.
    app.backend(build::modules(&[("blockchain_module", "loaded")]));

    assert_eq!(
        app.module_status("blockchain_module"),
        "loaded",
        "a poll result was reported as a load we were performing\n{}",
        app.screen(),
    );
}

/// The session this whole increment exists for. The blockchain module
/// aborted, took the daemon with it, and the respawn truncated the log its
/// `critical` line was written to — so by the time anyone looked, the only
/// artefact left was a row reading "not loaded". It has to read `crashed`,
/// on the strength of the transition alone.
#[tokio::test(start_paused = true)]
async fn a_crash_survives_the_restart_that_destroyed_its_log() {
    let mut app = App::fresh();

    app.online(&[]);
    app.backend(build::modules(&[
        ("chat_module", "loaded"),
        ("delivery_module", "loaded"),
        ("blockchain_module", "loaded"),
    ]));

    // Everything below the truncation: the log lines are gone with the
    // file, so nothing here tells the app a module died.
    app.script(build::restart(
        &["chat_module", "delivery_module", "blockchain_module"],
        &[
            ("chat_module", "loaded"),
            ("delivery_module", "loaded"),
            ("blockchain_module", "not_loaded"),
        ],
    ));

    assert_eq!(
        app.module_status("blockchain_module"),
        "crashed",
        "the module that took the daemon down reads as merely idle\n{}",
        app.screen(),
    );
    assert_eq!(
        app.alarmed_modules(),
        vec!["Blockchain".to_owned()],
        "\nscreen:\n{}\n",
        app.screen(),
    );
    assert_eq!(app.module_status("chat_module"), "loaded");
    assert_eq!(app.module_status("delivery_module"), "loaded");
}

/// The other side of the same rule, and the one that keeps it usable. A
/// restart takes every module away at once — legitimately, and as a *known*
/// event — so the modules that come back may not be mistaken for four
/// simultaneous deaths, and a module that was idle before it must not be
/// buried by it either.
#[tokio::test(start_paused = true)]
async fn a_restart_everything_survives_leaves_no_module_looking_crashed() {
    let mut app = App::fresh();

    app.online(&[]);
    app.backend(build::modules(&[
        ("chat_module", "loaded"),
        ("delivery_module", "loaded"),
        ("blockchain_module", "not_loaded"),
    ]));

    app.script(build::restart(
        &["chat_module", "delivery_module", "blockchain_module"],
        &[
            ("chat_module", "loaded"),
            ("delivery_module", "loaded"),
            ("blockchain_module", "not_loaded"),
        ],
    ));

    assert_eq!(
        app.alarmed_modules(),
        Vec::<String>::new(),
        "an ordinary restart painted the sidebar red\n{}",
        app.screen(),
    );
    assert_eq!(app.module_status("chat_module"), "loaded");
    assert_eq!(
        app.module_status("blockchain_module"),
        "not loaded",
        "a module that was already idle was buried by the restart\n{}",
        app.screen(),
    );
}

/// A restart is otherwise invisible: it takes about four seconds, the fresh
/// daemon reports an unremarkable module set, and the log that would have
/// explained it has been truncated by the respawn. The user is left to
/// account for a sidebar that emptied and refilled on its own — so the strip
/// says what happened, quietly, and can be dismissed.
#[tokio::test(start_paused = true)]
async fn a_backend_that_restarted_underneath_the_user_says_so() {
    let mut app = App::fresh();

    app.online(&[]);
    app.backend(build::modules(&[("chat_module", "loaded")]));

    assert!(
        !app.shows("restarted"),
        "nothing has restarted yet\n{}",
        app.screen(),
    );

    app.script(build::restart(
        &["chat_module"],
        &[("chat_module", "loaded")],
    ));

    assert!(
        app.shows("The backend restarted"),
        "a restart the user lived through left no trace\n{}",
        app.screen(),
    );

    app.script(build::restart(
        &["chat_module"],
        &[("chat_module", "loaded")],
    ));

    assert!(
        app.shows("The backend restarted 2 times"),
        "the second restart in a session says less than the first\n{}",
        app.screen(),
    );

    app.dismiss_restart_notice();

    assert!(
        !app.shows("The backend restarted"),
        "a dismissed notice came back\n{}",
        app.screen(),
    );

    // Dismissing is per restart, not for good: the next one is news again.
    app.script(build::restart(
        &["chat_module"],
        &[("chat_module", "loaded")],
    ));

    assert!(
        app.shows("The backend restarted 3 times"),
        "{}",
        app.screen()
    );
}

/// The crash the sidebar exists to show, arriving during the one event that
/// used to switch the whole inference off. `logos-chat` re-emits
/// `Phase(InitialisingChat)` for every live `delivery_state_changed`
/// carrying `initialising` — an ordinary blip on a backend that is up and
/// answering polls — so treating that phase as "the stack is being
/// assembled" let a real death be reported as `not loaded` and then lost for
/// the rest of the run.
#[tokio::test(start_paused = true)]
async fn a_delivery_blip_does_not_hide_a_module_dying_behind_it() {
    let mut app = App::fresh();

    app.online(&[]);
    app.backend(build::modules(&[
        ("chat_module", "loaded"),
        ("delivery_module", "loaded"),
        ("blockchain_module", "loaded"),
    ]));

    app.script(build::delivery_blip());

    // Nothing restarted; the daemon serves this poll itself.
    app.backend(build::modules(&[
        ("chat_module", "loaded"),
        ("delivery_module", "loaded"),
        ("blockchain_module", "not_loaded"),
    ]));

    assert_eq!(
        app.module_status("blockchain_module"),
        "crashed",
        "a delivery blip was read as a rebuild and swallowed a death\n{}",
        app.screen(),
    );
    assert_eq!(
        app.module_status("logoscore"),
        "loaded",
        "the daemon answered this very poll and was called loading\n{}",
        app.screen(),
    );
    assert_eq!(
        app.module_status("chat_module"),
        "loaded",
        "a delivery blip is not chat being reloaded\n{}",
        app.screen(),
    );

    app.script(build::delivery_recovered());
    app.backend(build::modules(&[
        ("chat_module", "loaded"),
        ("delivery_module", "loaded"),
        ("blockchain_module", "not_loaded"),
    ]));

    assert_eq!(
        app.alarmed_modules(),
        vec!["Blockchain".to_owned()],
        "the crash was forgotten once delivery came back\n{}",
        app.screen(),
    );
}

/// `Loading` is a claim about work the app is doing, and the app loads
/// exactly chat's closure. A module an operator loaded by hand is carried
/// across a restart only so its absence can be read afterwards — nothing
/// will reload it, so a row promising "not yet" could never be kept.
#[tokio::test(start_paused = true)]
async fn a_module_nothing_reloads_is_not_called_loading_by_the_restart() {
    let mut app = App::fresh();

    app.online(&[]);

    // `logoscore load-module blockchain_module`, from a terminal we know
    // nothing about.
    app.backend(build::modules(&[
        ("chat_module", "loaded"),
        ("delivery_module", "loaded"),
        ("blockchain_module", "loaded"),
    ]));

    app.script(build::loading_module());
    app.backend(build::modules(&[
        ("chat_module", "not_loaded"),
        ("delivery_module", "not_loaded"),
        ("blockchain_module", "not_loaded"),
    ]));

    assert_eq!(app.module_status("chat_module"), "loading");
    assert_eq!(
        app.module_status("blockchain_module"),
        "not loaded",
        "the app claimed to be loading a module it never loads\n{}",
        app.screen(),
    );

    // Carrying it was still the point: it was up, and it did not come back.
    app.script(build::startup_ladder());
    app.backend(build::modules(&[
        ("chat_module", "loaded"),
        ("delivery_module", "loaded"),
        ("blockchain_module", "not_loaded"),
    ]));

    assert_eq!(
        app.module_status("blockchain_module"),
        "crashed",
        "\nscreen:\n{}\n",
        app.screen(),
    );
}

/// The end of the road, and the one place the inference has nowhere else to
/// run: when the ladder gives up there is no daemon left to poll, so
/// whatever the rows say now is final. A module that was running when the
/// backend went down has to be left looking dead rather than merely idle —
/// "not loaded" is the exact row text the original report complained about.
#[tokio::test(start_paused = true)]
async fn a_backend_that_gave_up_leaves_the_module_that_died_looking_dead() {
    let mut app = App::fresh();

    app.online(&[]);
    app.backend(build::modules(&[
        ("chat_module", "loaded"),
        ("delivery_module", "loaded"),
        ("blockchain_module", "loaded"),
        ("capability_module", "not_loaded"),
    ]));

    app.script(build::restart_exhausted(3));

    assert_eq!(
        app.module_status("blockchain_module"),
        "crashed",
        "the backend gave up and the row went back to reading idle\n{}",
        app.screen(),
    );
    assert_eq!(
        app.module_status("capability_module"),
        "not loaded",
        "a module that was already idle was buried by the failure\n{}",
        app.screen(),
    );
    assert_eq!(
        app.alarmed_modules(),
        vec![
            "Chat".to_owned(),
            "Delivery".to_owned(),
            "Blockchain".to_owned(),
        ],
        "\nscreen:\n{}\n",
        app.screen(),
    );
}

/// The other half of the same rule, so the fix cannot be "call everything
/// crashed". A first run that never gets a daemon up has modules that were
/// asked to load and never appeared — they failed to load, which is not the
/// same fact as having died, and a sidebar full of error marks on a backend
/// that has never been up is a lie about processes that never ran.
#[tokio::test(start_paused = true)]
async fn a_first_run_that_never_starts_accuses_nothing_of_dying() {
    let mut app = App::fresh();

    app.script(build::loading_module());
    app.backend(build::modules(&[
        ("chat_module", "not_loaded"),
        ("delivery_module", "not_loaded"),
        ("blockchain_module", "not_loaded"),
    ]));

    assert_eq!(app.module_status("chat_module"), "loading");

    app.script(build::restart_exhausted(3));

    assert_eq!(
        app.alarmed_modules(),
        Vec::<String>::new(),
        "modules that never ran were accused of crashing\n{}",
        app.screen(),
    );
    assert_eq!(app.module_status("chat_module"), "not loaded");
}
