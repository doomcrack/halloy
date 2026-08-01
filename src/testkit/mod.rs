//! Headless UI harness: drives the real [`Frigicom`] update loop with
//! scripted backend updates on a virtual clock, and reads the result back
//! as strings. No window, no GPU, no daemon, no dylib — plain `cargo test`.
//!
//! The three seams it owns:
//!
//! - **Persistence.** [`App::restore`] takes the previous run's
//!   [`data::Dashboard`] in memory, so a test never reads the user's
//!   `dashboard.json.gz`. Every process-wide path is redirected into a
//!   temp dir by [`sandbox`] as a second line of defence.
//! - **The backend.** [`App::connect`] hands the app a control channel the
//!   harness owns, exactly as `stream::run` does with its first
//!   `Update::Controller`. Everything the UI asks of the backend is then
//!   readable with [`App::controls`].
//! - **Time.** Tasks are drained by hand and the clock only moves when a
//!   test says so ([`App::advance`]), so nothing here sleeps on the wall
//!   clock.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use futures::channel::mpsc;
use futures::{FutureExt, StreamExt};
use iced_runtime::Action;
use iced_runtime::futures::BoxStream;

use crate::buffer::Buffer;
use crate::stream::{self, Control, Update};
use crate::{
    Frigicom, Message, Screen, appearance, modal, notification, screen, system,
    window,
};

pub mod build;
pub mod cases;
pub mod render;

/// How many actions one drive step may produce before the harness calls it
/// a loop. Nothing in the app comes close; a runaway task would otherwise
/// hang the suite instead of failing it.
const PUMP_BUDGET: usize = 10_000;

/// The glyph `icon::error` draws, and the only status indicator in the
/// sidebar that is a text node at all — `icon::circle` is an SVG.
const ERROR_ICON: &str = "\u{E80D}";

/// Redirects `environment::{config_dir, data_dir}` into a per-process temp
/// directory. Any task the harness drains that decides to save the
/// dashboard writes there instead of the developer's real data dir.
static SANDBOX: LazyLock<PathBuf> = LazyLock::new(|| {
    let dir = std::env::temp_dir()
        .join("frigicom-testkit")
        .join(std::process::id().to_string());

    std::fs::create_dir_all(&dir).expect("create sandbox dir");

    // SAFETY: written once, from the first `App` constructed in the
    // process, and only ever read back through `environment::portable_dir`.
    unsafe {
        std::env::set_var("FRIGICOM_PORTABLE_DIR", &dir);
    }

    // `main` does this before iced starts; every `font::get` panics until
    // it has run, and the view layer calls it constantly.
    crate::font::set(&data::config::Font::default());

    dir
});

fn sandbox() {
    LazyLock::force(&SANDBOX);
}

/// A running app under test.
pub struct App {
    app: Frigicom,
    main: window::Id,
    sender: Option<mpsc::Sender<Control>>,
    controls: mpsc::Receiver<Control>,
    control_log: Vec<String>,
    deferred: Vec<BoxStream<Action<Message>>>,
    effects: Vec<String>,
}

impl App {
    /// Cold start with nothing persisted — a first launch.
    pub fn fresh() -> Self {
        Self::restore(data::Dashboard::default())
    }

    /// Cold start restoring `persisted`, the seam a previous run left
    /// behind. Runs the real `Dashboard::restore` path and drains its
    /// startup tasks.
    pub fn restore(persisted: data::Dashboard) -> Self {
        Self::with_config(persisted, data::Config::default())
    }

    pub fn with_config(
        persisted: data::Dashboard,
        config: data::Config,
    ) -> Self {
        sandbox();

        let main = window::Id::unique();
        let main_window = window::Window::new(main);
        let (dashboard, commands) =
            screen::Dashboard::restore(persisted, &config, &main_window);
        let (notifications, stream) = notification::Notifications::new(&config);
        let (sender, controls) = mpsc::channel(stream::CONTROL_CAP);

        let app = Frigicom {
            version: data::Version::new(),
            screen: Screen::Dashboard(dashboard),
            current_mode: appearance::Mode::Dark,
            theme: crate::Theme::default(),
            session: data::Session::default(),
            backend: stream::Map::default(),
            guard: ipc::Acquired::Owner,
            config,
            modal: None,
            modal_drafts: modal::Drafts::default(),
            main_window,
            focused_window: Some(main),
            pending_logs: vec![],
            notifications,
            has_been_online: false,
            power: system::State::default(),
        };

        let mut harness = App {
            app,
            main,
            sender: Some(sender),
            controls,
            control_log: vec![],
            deferred: vec![],
            effects: vec![],
        };

        harness.pump(iced::Task::batch(vec![
            stream.map(Message::Notification),
            commands.map(Message::Dashboard),
        ]));

        harness
    }
}

/// Driving the app forward.
impl App {
    /// Feeds one message through the real `Frigicom::update` and drains
    /// every task it produces, recursively, until nothing else is ready.
    pub fn send(&mut self, message: Message) {
        let task = self.app.update(message);

        self.pump(task);
    }

    /// Feeds one backend update, exactly as the stream subscription does.
    pub fn backend(&mut self, update: Update) {
        self.send(Message::Stream(update));
    }

    pub fn script(&mut self, updates: impl IntoIterator<Item = Update>) {
        for update in updates {
            self.backend(update);
        }
    }

    /// Hands the app the control channel this harness owns — the real
    /// stream's first item. Until it lands, everything the UI asks of the
    /// backend is dropped, which is precisely the restored-pane hazard.
    pub fn connect(&mut self) {
        let sender = self.sender.take().expect("controller handed over twice");

        self.backend(Update::Controller(sender));
    }

    /// The whole startup ladder up to a snapshot of `ids`: controller,
    /// phases, `Ready`, then `ConversationsSnapshot`.
    pub fn online(&mut self, ids: &[&str]) {
        self.connect();
        self.script(build::startup());
        self.backend(build::snapshot(ids));
    }

    /// Opens `id` in a popout window. `Dashboard::restore` reaches the same
    /// state for every persisted `popout_panes` entry, but only by way of a
    /// real `window::open` — an `Action::Window` the harness can record and
    /// never answer, there being no window server to hand an id back.
    /// Feeding the `NewWindow` message the runtime would have produced
    /// lands the identical popout.
    pub fn popout(&mut self, id: &str) {
        let window = window::Id::unique();
        let pane = screen::dashboard::pane::Pane::new(Buffer::from_data(
            data::Buffer::Conversation(id.into()),
            self.dashboard().history(),
            iced::Size::default(),
            &self.app.config,
        ));

        self.send(Message::Dashboard(screen::dashboard::Message::NewWindow(
            window, pane,
        )));
    }

    /// Feeds a batch of raw daemon-log lines, exactly as the tail
    /// subscription does.
    pub fn module_logs(&mut self, raw: &[&str]) {
        let lines = raw.iter().copied().map(build::module_line).collect();

        self.send(Message::ModuleLogging(lines));
    }

    /// Feeds a batch of the app's own log records, exactly as the logging
    /// subscription does.
    pub fn app_logs(&mut self, records: &[(data::log::Level, &str)]) {
        let records = records
            .iter()
            .map(|(level, message)| build::log_record(*level, message))
            .collect();

        self.send(Message::Logging(records));
    }

    /// Opens a module's pane the way clicking its sidebar row does.
    pub fn open_module(&mut self, name: &str) {
        self.open(data::Buffer::Module(name.into()));
    }

    /// Opens the app's own log pane, the other buffer with no backlog to
    /// fetch and the one the `Partial` bug was first found in.
    pub fn open_logs(&mut self) {
        self.open(data::Buffer::Internal(data::buffer::Internal::Logs));
    }

    /// Clicking a sidebar row: opens `buffer`, or focuses the pane already
    /// showing it.
    pub fn open(&mut self, buffer: data::Buffer) {
        self.send(Message::Dashboard(screen::dashboard::Message::Sidebar(
            screen::dashboard::sidebar::Message::New(buffer),
        )));
    }

    /// Swaps the focused pane's buffer for `buffer`, the way the sidebar's
    /// "replace pane" entry does. The pane the buffer left stops being
    /// tracked, which is what makes this the way to test that a history
    /// nothing can refetch survives its pane closing.
    pub fn replace_pane(&mut self, buffer: data::Buffer) {
        self.send(Message::Dashboard(screen::dashboard::Message::Sidebar(
            screen::dashboard::sidebar::Message::Replace(buffer),
        )));
    }

    /// Rebuilds the whole screen off disk the way a config reload does.
    /// `Frigicom::load_from_state` rereads `dashboard.json.gz`, so
    /// `persisted` is what comes back — stale ids and all — while the live
    /// session is carried across. The runtime arrives here by way of
    /// `Task::perform(Config::load(), Message::ScreenConfigReloaded)`;
    /// feeding the message it would have produced lands the identical
    /// rebuild without a config file to read.
    ///
    /// The one test seam that touches the sandbox on disk: the file is
    /// process-wide, but only a `Message::Tick` makes the app write it and
    /// the harness never sends one, so nothing else in the suite races for
    /// it.
    pub async fn reload_config(&mut self, persisted: data::Dashboard) {
        persisted
            .save()
            .await
            .expect("persist the dashboard on disk");

        let config = self.app.config.clone();

        self.send(Message::ScreenConfigReloaded(Ok(config)));
    }

    /// Moves the virtual clock and re-drains whatever was waiting on it.
    /// Requires `#[tokio::test(start_paused = true)]`.
    pub async fn advance(&mut self, duration: Duration) {
        tokio::time::advance(duration).await;

        self.drain();
    }

    fn pump(&mut self, task: iced::Task<Message>) {
        self.defer(task);
        self.drain();
    }

    fn defer(&mut self, task: iced::Task<Message>) {
        if let Some(stream) = iced_runtime::task::into_stream(task) {
            self.deferred.push(stream);
        }
    }

    fn drain(&mut self) {
        for _ in 0..PUMP_BUDGET {
            let Some(action) = self.next_action() else {
                return;
            };

            match action {
                Action::Output(message) => {
                    let task = self.app.update(message);
                    self.defer(task);
                }
                other => self.effects.push(effect_label(&other)),
            }
        }

        panic!(
            "task pump did not settle after {PUMP_BUDGET} actions; last \
             effects: {:?}",
            self.effects.iter().rev().take(8).collect::<Vec<_>>()
        );
    }

    fn next_action(&mut self) -> Option<Action<Message>> {
        let mut index = 0;

        while index < self.deferred.len() {
            match self.deferred[index].next().now_or_never() {
                Some(Some(action)) => return Some(action),
                Some(None) => {
                    let _ = self.deferred.remove(index);
                }
                None => index += 1,
            }
        }

        None
    }
}

/// Reading state back. Everything is a `String` or a `Vec<String>` so an
/// assertion failure can be pasted back in as the expected value.
impl App {
    /// The dashboard exactly as it would be persisted right now — through
    /// the same `From<&Dashboard>` that writes `dashboard.json.gz`.
    pub fn persisted(&self) -> data::Dashboard {
        data::Dashboard::from(self.dashboard())
    }

    /// The pane tree, e.g. `split-v(convo:c1, convo:c2)`.
    pub fn layout(&self) -> String {
        layout_label(&self.persisted().pane)
    }

    /// Every open pane's buffer key, main window first, then popouts.
    pub fn panes(&self) -> Vec<String> {
        let persisted = self.persisted();
        let mut keys = vec![];

        pane_keys(&persisted.pane, &mut keys);

        for popout in &persisted.popout_panes {
            pane_keys(popout, &mut keys);
        }

        keys
    }

    /// The focused pane's buffer key, or `none` when focus landed nowhere.
    pub fn focus(&self) -> String {
        self.persisted()
            .focus_buffer
            .map_or_else(|| "none".to_owned(), |buffer| buffer.key())
    }

    /// Conversation ids the app currently believes exist.
    pub fn live_conversations(&self) -> Vec<String> {
        self.app
            .session
            .conversations
            .iter()
            .map(|conversation| conversation.id.to_string())
            .collect()
    }

    /// Pane buffer keys naming a conversation the backend never sent — the
    /// motivating bug, stated as a query.
    pub fn stale_panes(&self) -> Vec<String> {
        let live: Vec<String> = self
            .live_conversations()
            .into_iter()
            .map(|id| format!("convo:{id}"))
            .collect();

        self.panes()
            .into_iter()
            .filter(|key| key.starts_with("convo:") && !live.contains(key))
            .collect()
    }

    /// Drains everything the UI has asked of the backend since the last
    /// call, e.g. `["LoadMessages(c1)", "LoadMembers(c1)"]`.
    pub fn controls(&mut self) -> Vec<String> {
        while let Ok(control) = self.controls.try_recv() {
            self.control_log.push(control_label(&control));
        }

        std::mem::take(&mut self.control_log)
    }

    /// Every line a module's pane is rendering, oldest first.
    ///
    /// Read from the history rather than from [`App::text`] because a log row
    /// is a `selectable_text`, which reports itself to a traversal only while
    /// something is selected — log rows leave no text node behind. This is
    /// the exact view the pane draws from, and it is empty for a history
    /// still `Partial`, which is the shape of the bug these panes must not
    /// have. What the screen *can* answer is whether the pane fell back to a
    /// placeholder, which is what a `Partial` history would still be showing.
    pub fn module_lines(&self, name: &str) -> Vec<String> {
        self.lines(&data::history::Kind::Module(name.into()))
    }

    /// Every record the app's own log pane is rendering. Read the same way,
    /// and for the same reason, as [`App::module_lines`].
    pub fn log_lines(&self) -> Vec<String> {
        self.lines(&data::history::Kind::Logs)
    }

    /// Modules whose sidebar row is showing the alarm indicator.
    ///
    /// A module row's entire status report is its icon, and exactly one
    /// status changes that icon's *shape*: `Crashed` swaps the circle for an
    /// error mark. Shape is the half a text-only view can see — the circle is
    /// an SVG and leaves no text node behind — so `Loaded` and `NotLoaded`,
    /// which differ only in the circle's colour, are told apart through
    /// [`App::module_status`] instead.
    pub fn alarmed_modules(&self) -> Vec<String> {
        let text = self.text();

        text.iter()
            .zip(text.iter().skip(1))
            .filter(|(icon, _)| icon.as_str() == ERROR_ICON)
            .map(|(_, name)| name.clone())
            .collect()
    }

    /// How many places on screen can be typed into. Zero is what "read-only"
    /// means for a pane that has no composer to disable.
    pub fn composers(&self) -> usize {
        render::composers(&self.app, self.main)
    }

    /// What the app believes a module's status is, e.g. `crashed`.
    pub fn module_status(&self, name: &str) -> String {
        self.app.session.module(&name.into()).map_or_else(
            || "unknown to the app".to_owned(),
            |module| module.status.label().to_owned(),
        )
    }

    /// What a module row's pulse column reads, by position: the row is an
    /// icon, the module's name, then the events counted in the window the
    /// last report closed.
    ///
    /// An empty column contributes no text node at all, so the count is
    /// recognised by shape — the next thing on screen belongs to another
    /// row, and nothing else in a module row is a number.
    pub fn module_pulse(&self, display_name: &str) -> String {
        let text = self.text();
        let row = text
            .iter()
            .position(|line| line == display_name)
            .unwrap_or_else(|| panic!("no row for {display_name}"));

        text.get(row + 1)
            .filter(|pulse| {
                pulse.chars().all(|glyph| glyph.is_ascii_digit())
                    || *pulse == "999+"
            })
            .cloned()
            .unwrap_or_default()
    }

    pub fn unread(&self, id: &str) -> usize {
        self.dashboard()
            .history()
            .unread_count(&data::history::Kind::Conversation(id.into()))
    }

    pub fn draft(&self, id: &str) -> String {
        self.dashboard()
            .history()
            .input(&data::conversation::ConvoId::from(id))
            .draft_message
            .to_owned()
    }

    pub fn messages(&self, id: &str) -> Vec<String> {
        self.lines(&data::history::Kind::Conversation(id.into()))
    }

    /// Every message a history would hand a pane right now, oldest first.
    /// A history still `Partial` renders nothing, so this is empty for one —
    /// which is exactly the shape of the bug the module panes must not have.
    fn lines(&self, kind: &data::history::Kind) -> Vec<String> {
        self.dashboard()
            .history()
            .get_messages(kind, None)
            .map_or_else(Vec::new, |view| {
                view.old_messages
                    .iter()
                    .chain(view.new_messages.iter())
                    .map(|message| message.text().into_owned())
                    .collect()
            })
    }

    /// Every visible text node of the real `view()`, in tree order.
    pub fn text(&self) -> Vec<String> {
        render::text(&self.app, self.main)
    }

    /// [`App::text`] joined by newlines, for one-glance failure output.
    pub fn screen(&self) -> String {
        self.text().join("\n")
    }

    pub fn shows(&self, needle: &str) -> bool {
        self.text().iter().any(|line| line.contains(needle))
    }

    fn dashboard(&self) -> &screen::Dashboard {
        match &self.app.screen {
            Screen::Dashboard(dashboard) => dashboard,
            _ => panic!("app is not on the dashboard screen"),
        }
    }
}

fn pane_keys(pane: &data::Pane, out: &mut Vec<String>) {
    match pane {
        data::Pane::Split { a, b, .. } => {
            pane_keys(a, out);
            pane_keys(b, out);
        }
        data::Pane::Buffer { buffer } => out.push(buffer.key()),
        data::Pane::Empty => out.push("empty".to_owned()),
    }
}

fn layout_label(pane: &data::Pane) -> String {
    match pane {
        data::Pane::Split { axis, a, b, .. } => {
            let axis = match axis {
                data::pane::Axis::Horizontal => "split-h",
                data::pane::Axis::Vertical => "split-v",
            };

            format!("{axis}({}, {})", layout_label(a), layout_label(b))
        }
        data::Pane::Buffer { buffer } => buffer.key(),
        data::Pane::Empty => "empty".to_owned(),
    }
}

fn control_label(control: &Control) -> String {
    match control {
        Control::SendMessage { convo_id, content } => {
            format!("SendMessage({convo_id}, {content:?})")
        }
        Control::CreateConversation { peer_address } => {
            format!("CreateConversation({peer_address})")
        }
        Control::CreateGroup { name, .. } => format!("CreateGroup({name})"),
        Control::AddGroupMember {
            convo_id,
            peer_address,
        } => format!("AddGroupMember({convo_id}, {peer_address})"),
        Control::LoadMessages { convo_id } => {
            format!("LoadMessages({convo_id})")
        }
        Control::LoadMembers { convo_id } => format!("LoadMembers({convo_id})"),
        Control::SetNickname { convo_id, nickname } => {
            format!("SetNickname({convo_id}, {nickname})")
        }
        Control::DeleteConversation { convo_id } => {
            format!("DeleteConversation({convo_id})")
        }
        Control::Resync => "Resync".to_owned(),
        Control::RefreshModules => "RefreshModules".to_owned(),
        Control::Quit => "Quit".to_owned(),
    }
}

fn effect_label(action: &Action<Message>) -> String {
    match action {
        Action::Output(_) => "Output",
        Action::Widget(_) => "Widget",
        Action::Clipboard(_) => "Clipboard",
        Action::Window(_) => "Window",
        Action::System(_) => "System",
        Action::Font(_) => "Font",
        Action::Image(_) => "Image",
        Action::Backend(_) => "Backend",
        Action::Event { .. } => "Event",
        Action::Tick => "Tick",
        Action::Reload => "Reload",
        Action::Exit => "Exit",
    }
    .to_owned()
}
