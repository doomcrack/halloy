# Testing

Frigicom's tests are layered so that almost everything can be asserted in a
plain, headless `cargo test` — no daemon, no `liblogos_protocol`, no GPU, no
window server, and no wall-clock sleeps.

## The layers

| Layer | Where | What it covers | Cost |
| --- | --- | --- | --- |
| **L0 domain** | `cargo test -p data` | `Session::apply`, `history::Manager`, buffer/config serde | ~0 ms |
| **L1 app loop** | `cargo test -p frigicom` (`src/testkit`) | the real `Frigicom::update` + real `screen::Dashboard` fed scripted `stream::Update`s; layout, focus, unread, drafts, and everything the UI asks of the backend | ~1 ms per case |
| **L2 view as text** | same binary (`src/testkit/render.rs`) | the real `Frigicom::view` laid out headlessly, reduced to the list of visible text nodes | ~0.5 ms per call |
| **L3 backend session** | `cargo test -p logos-chat` | `session::run` against `Driver::Fake` — the wire ladder, reconnects, coalescing | ~50 ms |
| **live** | `cargo test -p logos-chat --features live -- --ignored` | a real daemon and module | seconds to minutes |

L1 is the centre of gravity. Most interesting bugs in this app are
*reconciliation* bugs — persisted state meeting a backend snapshot — and they
live in the update loop, not in the pixels.

There is deliberately **no pixel/snapshot layer**. Frigicom enables both `wgpu`
and `tiny-skia` on iced, so `iced_test` picks a renderer at runtime and writes a
per-renderer baseline on first sight; a snapshot suite would therefore go
silently green on any machine whose renderer differs from the one that produced
the committed baseline. L2 asserts on text, which is renderer-independent.

## Running

```sh
cargo test -p frigicom          # the UI harness (plus the other bin tests)
cargo test -p data              # domain
cargo check --workspace
cargo clippy -p frigicom --no-deps --all-targets
```

Nothing above needs `LOGOS_PROTOCOL_ROOT`, the dev shell, or a display. A warm
run of the whole UI suite is under a second.

To run one case, or one class:

```sh
cargo test -p frigicom testkit::cases::reconciliation
cargo test -p frigicom empty_thread_waits_out
```

## Writing a case

Cases live in `src/testkit/cases/`, one module per class
(`reconciliation`, `gating`, `ordering`, `drafts`). A case is usually ten lines:

```rust
use crate::testkit::{App, build};

#[tokio::test(start_paused = true)]
async fn a_snapshot_replaces_the_conversation_list() {
    // the dashboard a previous run left behind
    let mut app = App::restore(build::persisted(build::pane("c1"), Some("c1")));

    // controller -> phases -> Ready -> ConversationsSnapshot(["c1"])
    app.online(&["c1"]);

    assert_eq!(app.layout(), "convo:c1");
    assert_eq!(app.focus(), "convo:c1");
    assert!(app.controls().contains(&"LoadMessages(c1)".to_owned()));
}
```

`#[tokio::test(start_paused = true)]` is required: it is what makes the load
windows (and any other `tokio::time` timer) virtual.

To add a new class, drop a module into `src/testkit/cases/` and declare it in
`src/testkit/cases/mod.rs`.

### Driving

| Call | Effect |
| --- | --- |
| `App::fresh()` | cold start, nothing persisted |
| `App::restore(dashboard)` | cold start restoring a previous run's dashboard |
| `App::with_config(dashboard, config)` | the same with a non-default config |
| `app.connect()` | hand over the control channel (the stream's first item) |
| `app.script(build::startup())` | the phase ladder and `Ready` |
| `app.backend(update)` | one `stream::Update` |
| `app.online(&["c1"])` | `connect` + `startup` + snapshot, in one line |
| `app.popout("c1")` | open `c1` in a popout window (see below) |
| `app.reload_config(dashboard).await` | rebuild the screen off disk, as a config reload does |
| `app.send(message)` | any `Message`, straight into `Frigicom::update` |
| `app.advance(Duration::from_millis(200)).await` | move the virtual clock |

`app.reload_config` is the second restore path: a config reload throws the whole
screen away and rebuilds it with `load_from_state`, which rereads
`dashboard.json.gz` while the live session is carried across. It writes the
dashboard you hand it into the sandbox data dir first, so the rebuilt screen
comes back with exactly that state — dead ids and all — then feeds the
`ScreenConfigReloaded` message the runtime would have produced.

`app.popout` exists because a persisted `popout_panes` entry only becomes a
window through a real `window::open` — an `Action::Window` the harness records
and cannot answer, there being no window server to hand an id back. It feeds the
`NewWindow` message the runtime would have produced, landing the same popout, so
a case can assert on a second window at all.

Every call drives the real update loop to quiescence: the `Task` it returns is
drained, the messages it emits are fed back in, and so on recursively. If that
ever fails to settle the harness panics with the last effects it saw rather than
hanging.

### Asserting

Everything reads back as a `String` or a `Vec<String>`, so a failure diff can be
pasted straight back in as the expected value.

| Call | Returns |
| --- | --- |
| `app.layout()` | `"split-v(convo:c1, convo:c2)"` |
| `app.panes()` | `["convo:c1", "convo:c2"]`, main window then popouts |
| `app.focus()` | `"convo:c2"` or `"none"` |
| `app.persisted()` | the `data::Dashboard` that would be written to disk right now |
| `app.live_conversations()` | ids the app believes exist |
| `app.stale_panes()` | pane keys naming a conversation the backend never sent |
| `app.controls()` | `["LoadMessages(c1)", "LoadMembers(c1)"]` — drains |
| `app.unread("c1")`, `app.draft("c1")`, `app.messages("c1")` | history state |
| `app.text()`, `app.screen()`, `app.shows("…")` | L2: what is actually on screen |

`app.layout()` / `app.focus()` / `app.persisted()` all go through the same
`From<&Dashboard> for data::Dashboard` that writes `dashboard.json.gz`, so an
assertion about layout is an assertion about what the next launch will restore.

### Fixtures

`src/testkit/build.rs` has the one-liners: `pane`, `split_v`, `persisted`,
`without_sidebar`, `startup`, `snapshot`, `conversation`, `messages_loaded`,
`received`, `offline`, `send_failed`. Add builders there rather than hand-rolling
wire types in a case.

## How time works

`src/buffer/conversation.rs` keeps its grace/settle windows on
`tokio::time::Instant`, the same clock its redraw timer already used. Outside a
tokio runtime that is `std::time::Instant`, so the shipped binary is unchanged;
under `#[tokio::test(start_paused = true)]` it is the virtual clock, and
`app.advance(…)` moves it.

That is the only reason a test can assert "the empty state has *not* appeared
yet" and then "now it has" without sleeping:

```rust
app.online(&["c1"]);
assert!(!app.shows("No messages yet"));       // inside the grace window

app.advance(Duration::from_millis(200)).await;
app.backend(build::messages_loaded("c1", &[]));
assert!(!app.shows("No messages yet"));       // inside the settle window

app.advance(Duration::from_millis(400)).await;
assert!(app.shows("No messages yet"));        // settled
```

Never use `std::thread::sleep` or `tokio::time::sleep` in a case. If something
seems to need it, it needs `advance` instead.

## Reading a failure

L1 assertions print state, not stack traces. A reconciliation failure looks like
this:

```
assertion `left == right` failed:
layout: split-v(convo:old-1, convo:old-2)
focus:  convo:old-2
live:   ["new-1"]
screen:
…
Conversation old-1
Conversation old-2

  left: ["convo:old-1", "convo:old-2"]
 right: []
```

Read it top down: `layout`/`focus` are what will be persisted, `live` is what the
backend actually has, `screen` is what the user sees. The `left`/`right` diff is
the assertion itself.

When you add an assertion, attach the same context to it — `app.layout()`,
`app.focus()`, `app.screen()` — so the next failure is self-explaining.

## Notes for an automated contributor

- After changing anything under `src/` or `data/`, run
  `cargo test -p frigicom -p data` first. It is the fastest signal in the repo
  (well under a second warm) and it exercises the real update loop.
- Before finishing, run the full gate:
  `cargo check --workspace && cargo clippy -p frigicom --no-deps --all-targets && cargo test -p data -p logos-chat -p frigicom`.
- If a case panics with `task pump did not settle`, some `Task` is emitting
  messages forever. The panic prints the last effects it saw.
- If `app.shows("…")` matches when you did not expect it, remember the sidebar
  carries its own copies of several strings (a conversation with no preview also
  reads "No messages yet"). Build the fixture with `build::without_sidebar(…)`
  when the assertion is about the thread.
- L2 only sees `text` widgets. Message rows are `selectable_text` and do not
  appear in `app.text()`; assert on `app.messages("c1")` instead.
- Tests never touch the real config or data directory: `src/testkit` points
  `FRIGICOM_PORTABLE_DIR` at a per-process temp directory before the first `App`
  is built.
