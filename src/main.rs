#![allow(clippy::large_enum_variant, clippy::too_many_arguments)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appearance;
mod audio;
mod buffer;
mod emoji;
mod event;
mod font;
mod icon;
mod logger;
mod mock;
mod modal;
mod module_log;
mod notification;
mod open_url;
mod platform_specific;
mod screen;
mod stream;
mod system;
#[cfg(test)]
mod testkit;
mod unix_signal;
mod url;
mod widget;
mod window;

use std::env;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use appearance::{Theme, theme};
use data::address::Address;
use data::config::{self, Config, Runtime, runtime};
use data::conversation::{ConvoId, Kind as ConversationKind};
use data::dashboard::BufferAction;
use data::version::Version;
use data::{Notification, Url, environment, history, version};
use iced::widget::{column, container};
use iced::{Length, Subscription, Task, padding};
use screen::{dashboard, help};
use tokio_stream::wrappers::ReceiverStream;

use self::event::{Event, events};
use self::modal::Modal;
use self::notification::Notifications;
use self::stream::{ActionError, ChatEvent, Phase, Update};
use self::widget::{Element, text};
use self::window::Window;

/// Hard cap on [`Screen::Exit`]. Covers the session's own shutdown budget
/// (chat 5s + daemon 3s + supervisor grace 3s + kill grace 2s + logos
/// thread join 25s) with slack, so a backend that never answers
/// `Control::Quit` cannot park the app on a shutdown screen forever.
const EXIT_DEADLINE: Duration = Duration::from_secs(40);

/// How many times startup retries the handoff to an already-running
/// instance before giving up and claiming the state directory itself.
const HANDOFF_ATTEMPTS: u8 = 3;

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args();
    args.next();

    let version = args.next().is_some_and(|s| s == "--version" || s == "-V");

    if version {
        println!("frigicom {}", environment::formatted_version());

        return Ok(());
    }

    // Prepare the crypto provider before any TLS config is built. The
    // workspace pins reqwest's `rustls-no-provider`, so every
    // `Client::builder().build()` panics until a provider is installed —
    // the update check and link previews both build one.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Prepare notifications.
    notification::prepare();

    let logs_config = Config::load_logs().unwrap_or_default();

    let log_stream = logger::setup(logs_config).expect("setup logging");
    log::info!("frigicom {} has started", environment::formatted_version());
    log::info!("config dir: {:?}", environment::config_dir());
    log::info!("data dir: {:?}", environment::data_dir());

    // spin up a single-threaded tokio runtime to run the config loading
    // tasks to completion; we don't want to wrap our whole program with a
    // runtime since iced starts its own.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let (config_load, window_load) = {
        rt.block_on(async {
            let config = Config::load().await;
            let window = data::Window::load().await;

            (config, window)
        })
    };

    // Futures have only been run via block_on, so we should be able to
    // shutdown_background without leaks
    rt.shutdown_background();

    // If the config fails to load, then attempt to load the font alone
    // in order to provide UI as close to configured as possible.
    // Particularly of use while the font cannot be changed while
    // the application is running.
    let font_config = config_load
        .as_ref()
        .ok()
        .map(|config| config.font.clone())
        .or(Config::load_font())
        .unwrap_or_default();

    // DANGER ZONE - font must be set using config
    // before we do any iced related stuff w/ it
    font::set(&font_config);

    let destination = data::Url::find_in(std::env::args());

    // Single-instance guard. One app per backend state directory, or the
    // second logoscore daemon reaps the first one's. It has to settle
    // before iced — and with it the backend session — starts.
    let state_dir = config_load
        .as_ref()
        .ok()
        .and_then(|config| config.logos.instance_dir.clone())
        .unwrap_or_else(environment::logos_instance_dir);

    let payload = destination
        .as_ref()
        .map_or_else(|| ipc::FOCUS.to_owned(), ToString::to_string);

    // `AlreadyRunning` only means a peer answered the socket probe. A
    // handoff that does not land means the owner exited in between, so
    // re-acquire — the next probe reclaims the dead socket — instead of
    // returning with no window open and the url dropped.
    let mut attempts = HANDOFF_ATTEMPTS;

    let guard = loop {
        match ipc::acquire(&state_dir) {
            ipc::Acquired::AlreadyRunning => {
                log::info!("another instance owns {state_dir:?}, handing over");

                if ipc::connect_and_send(&state_dir, &payload) {
                    return Ok(());
                }

                attempts -= 1;

                if attempts == 0 {
                    log::error!(
                        "could not hand {payload} to the instance owning \
                         {state_dir:?}; starting anyway"
                    );

                    // Something kept answering the socket, so this run
                    // starts alongside an owner it could not reach: it
                    // holds no claim on the state dir.
                    break ipc::Acquired::Unguarded;
                }
            }
            acquired => break acquired,
        }
    };

    let settings = settings(&config_load, &font_config);
    let log_stream = Mutex::new(Some(log_stream));

    iced::daemon(
        move || {
            let log_stream = log_stream
                .lock()
                .unwrap()
                .take()
                .expect("will only panic if using iced_devtools");

            Frigicom::new(
                config_load.clone(),
                window_load.clone(),
                destination.clone(),
                log_stream,
                // we start with an unspecified mode because we are guaranteed to
                // receive a message from mundy containing the correct mode on startup.
                appearance::Mode::Unspecified,
                guard,
            )
        },
        Frigicom::update,
        Frigicom::view,
    )
    .title(Frigicom::title)
    .theme(Frigicom::theme)
    .scale_factor(Frigicom::scale_factor)
    .subscription(Frigicom::subscription)
    .settings(settings)
    .run()
    .inspect_err(|err| log::error!("{err}"))?;

    Ok(())
}

fn settings(
    config_load: &Result<Config, config::Error>,
    font_config: &config::Font,
) -> iced::Settings {
    let default_text_size =
        font_config.size.map_or(theme::TEXT_SIZE, f32::from);

    let runtime = config_load
        .as_ref()
        .map_or_else(|_| Runtime::default(), |config| config.runtime);

    iced::Settings {
        default_font: font::MONO.clone().into(),
        default_text_size: default_text_size.into(),
        backend: backend_from_config(runtime.backend),
        power_preference: power_preference_from_config(
            runtime.power_preference,
        ),
        id: None,
        antialiasing: runtime.antialiasing,
        fonts: font::load(),
        vsync: runtime.vsync,
        metrics_hinting: runtime.metrics_hinting,
    }
}

fn backend_from_config(backend: runtime::Backend) -> iced::Backend {
    match backend {
        runtime::Backend::Best => iced::Backend::Best,
        runtime::Backend::Hardware(api) => iced::Backend::Hardware(match api {
            runtime::HardwareApi::Best => iced::backend::Api::Best,
            runtime::HardwareApi::Vulkan => iced::backend::Api::Vulkan,
            runtime::HardwareApi::Metal => iced::backend::Api::Metal,
            runtime::HardwareApi::DirectX12 => iced::backend::Api::DirectX12,
            runtime::HardwareApi::OpenGL => iced::backend::Api::OpenGL,
            runtime::HardwareApi::WebGPU => iced::backend::Api::WebGPU,
        }),
        runtime::Backend::Software => iced::Backend::Software,
    }
}

fn power_preference_from_config(
    power_preference: runtime::PowerPreference,
) -> iced::PowerPreference {
    match power_preference {
        runtime::PowerPreference::None => iced::PowerPreference::None,
        runtime::PowerPreference::LowPower => iced::PowerPreference::LowPower,
        runtime::PowerPreference::HighPerformance => {
            iced::PowerPreference::HighPerformance
        }
    }
}

fn configure_runtime(runtime: Runtime) -> Task<Message> {
    iced::backend::configure(iced::backend::Settings {
        backend: backend_from_config(runtime.backend),
        power_preference: power_preference_from_config(
            runtime.power_preference,
        ),
        antialiasing: runtime.antialiasing,
        vsync: runtime.vsync,
    })
    .map(Message::RuntimeConfigured)
}

struct Frigicom {
    version: Version,
    screen: Screen,
    current_mode: appearance::Mode,
    theme: Theme,
    config: Config,
    /// Domain fold of the backend session (delivery, identity,
    /// conversations).
    session: data::Session,
    /// Control handle to the running backend session.
    backend: stream::Map,
    /// Outcome of the single-instance guard. Anything but [`Owner`] means
    /// a sibling instance may be live on the same state dir, so the
    /// backend must not reap the daemon it finds there.
    ///
    /// [`Owner`]: ipc::Acquired::Owner
    guard: ipc::Acquired,
    modal: Option<Modal>,
    /// Dialog drafts that survive a dismissed modal.
    modal_drafts: modal::Drafts,
    main_window: Window,
    focused_window: Option<window::Id>,
    pending_logs: Vec<data::log::Record>,
    notifications: Notifications,
    /// Whether delivery has ever been online this run — distinguishes
    /// `Connected` from `Reconnected` notifications.
    has_been_online: bool,
    power: system::State,
}

impl Frigicom {
    pub fn load_from_state(
        main_window: window::Id,
        config_load: Result<Config, config::Error>,
        current_mode: appearance::Mode,
        guard: ipc::Acquired,
    ) -> (Frigicom, Task<Message>) {
        let main_window = Window::new(main_window);

        let load_dashboard = |config: &Config| match data::Dashboard::load() {
            Ok(dashboard) => {
                screen::Dashboard::restore(dashboard, config, &main_window)
            }
            Err(error) => {
                if data::Dashboard::exists().is_ok_and(|exists| exists) {
                    log::warn!("failed to load dashboard: {error}");
                } else {
                    // Most likely this means it is the user's first launch,
                    // downgrade severity to info
                    log::info!("failed to load dashboard: {error}");
                }

                screen::Dashboard::empty(&main_window, config)
            }
        };

        let (mut screen, config, commands) = match config_load {
            Ok(config) => {
                let (screen, commands) = load_dashboard(&config);

                (
                    Screen::Dashboard(screen),
                    config,
                    commands.map(Message::Dashboard),
                )
            }
            Err(config::Error::ConfigMissing) => {
                let config = Config::default();
                let (screen, commands) = load_dashboard(&config);
                (
                    Screen::Dashboard(screen),
                    config,
                    commands.map(Message::Dashboard),
                )
            }
            Err(error) => (
                Screen::Help(screen::Help::new(error)),
                Config::default(),
                Task::none(),
            ),
        };

        let (notifications, stream) = Notifications::new(&config);

        let commands =
            Task::batch(vec![stream.map(Message::Notification), commands]);

        // Unguarded is survivable but not silent: a second instance on the
        // same state dir means two identities, and the backend deliberately
        // stops reaping the daemon it finds. Say so where the user looks.
        if let (ipc::Acquired::Unguarded, Screen::Dashboard(dashboard)) =
            (guard, &mut screen)
        {
            dashboard.report_error(
                "single-instance guard unavailable — another frigicom may be \
                 running on this data directory"
                    .to_owned(),
            );
        }

        (
            Frigicom {
                version: Version::new(),
                screen,
                current_mode,
                theme: current_mode.theme(&config.appearance.selected).into(),
                session: data::Session::default(),
                backend: stream::Map::default(),
                guard,
                config,
                modal: None,
                modal_drafts: modal::Drafts::default(),
                main_window,
                focused_window: None,
                pending_logs: vec![],
                notifications,
                has_been_online: false,
                power: system::State::default(),
            },
            commands,
        )
    }
}

pub enum Screen {
    Dashboard(screen::Dashboard),
    Help(screen::Help),
    /// Waiting for the backend to acknowledge `Control::Quit` with
    /// `Update::Stopped`, until `deadline` — past it the app exits
    /// regardless, rather than hanging on a shutdown screen.
    Exit {
        deadline: Instant,
    },
}

#[derive(Debug)]
pub enum Message {
    AppearanceReloaded(data::appearance::Appearance),
    ScreenConfigReloaded(Result<Config, config::Error>),
    Dashboard(dashboard::Message),
    Stream(stream::Update),
    Help(help::Message),
    Event(window::Id, Event),
    Tick(Instant),
    Version(Option<String>),
    Modal(modal::Message),
    RouteReceived(String),
    AppearanceChange(appearance::Mode),
    Window(window::Id, window::Event),
    WindowSettingsSaved(Result<(), window::Error>),
    WindowMaximizeChecked(bool),
    Logging(Vec<logger::Record>),
    /// A batch of tailed daemon-log lines, already parsed and attributed.
    /// Batched rather than per-line for the same reason [`Self::Logging`] is:
    /// a syncing node writes faster than a frame.
    ModuleLogging(Vec<data::module::tail::Line>),
    UnixSignal(i32),
    ConfigReloaded(Result<Config, config::Error>),
    RuntimeConfigured(Result<(), iced::backend::Error>),
    SystemInformation(iced::system::Information),
    Notification(notification::Event),
    System(system::Event),
}

impl Frigicom {
    fn save_main_window_settings(&self) -> Task<Message> {
        let main_window = self.main_window;

        // In multi-monitor layouts with a display above or offset from the
        // primary, `Moved` events can be missed. Query the current position
        // before saving.
        iced::window::position(main_window.id).then(move |position| {
            let mut main_window = main_window;
            if let Some(position) = position {
                main_window.position = Some(position);
            }

            Task::perform(
                data::Window::from(main_window).save(),
                Message::WindowSettingsSaved,
            )
        })
    }

    fn new(
        config_load: Result<Config, config::Error>,
        window_load: Result<data::Window, window::Error>,
        url_received: Option<data::Url>,
        log_stream: ReceiverStream<Vec<logger::Record>>,
        current_mode: appearance::Mode,
        guard: ipc::Acquired,
    ) -> (Frigicom, Task<Message>) {
        let data::Window {
            size,
            position,
            fullscreen,
            maximized,
        } = window_load.unwrap_or_default();

        let default_config = Config::default();
        let config = config_load.as_ref().unwrap_or(&default_config);
        let check_for_update_on_launch = config.check_for_update_on_launch;
        let window_size = iced::Size::new(
            config
                .window
                .initial_width
                .map_or(size.width, |width| width as f32),
            config
                .window
                .initial_height
                .map_or(size.height, |height| height as f32),
        );

        let (main_window, open_main_window) = window::open(window::Settings {
            size: fullscreen.unwrap_or(window_size),
            position: position
                .map(window::Position::Specific)
                .unwrap_or_default(),
            min_size: Some(window::MIN_SIZE),
            exit_on_close_request: false,
            fullscreen: fullscreen.is_some(),
            ..window::settings(config)
        });

        let (mut frigicom, command) = Frigicom::load_from_state(
            main_window,
            config_load,
            current_mode,
            guard,
        );

        frigicom.main_window.fullscreen = fullscreen;
        frigicom.main_window.maximized = maximized;
        frigicom.main_window.windowed_position = position;
        frigicom.main_window.windowed_size = size;

        let open_task = if maximized {
            open_main_window.then(move |_| window::maximize(main_window, true))
        } else {
            open_main_window.then(|_| Task::none())
        };

        let mut commands = vec![
            open_task,
            command,
            Task::stream(log_stream).map(Message::Logging),
        ];

        if check_for_update_on_launch {
            commands.push(Task::perform(
                version::latest_remote_version(),
                Message::Version,
            ));
        }

        if let Some(url) = url_received {
            commands.push(frigicom.handle_url(url));
        }

        (frigicom, Task::batch(commands))
    }

    fn handle_url(&mut self, url: Url) -> Task<Message> {
        match url {
            data::Url::Theme { styles, .. } => {
                if let Screen::Dashboard(dashboard) = &mut self.screen {
                    return dashboard
                        .preview_theme_in_editor(
                            styles,
                            &self.main_window,
                            &mut self.theme,
                            &self.config,
                        )
                        .map(Message::Dashboard);
                }
            }
            data::Url::Unknown(url) => {
                log::warn!("Received unknown url: {url}");
            }
        }

        Task::none()
    }

    fn title(&self, window_id: window::Id) -> String {
        let default = "Frigicom".to_owned();
        let s = match &self.screen {
            Screen::Dashboard(dashboard) => {
                match dashboard.focused_buffer_name(window_id, &self.session) {
                    Some(s) => s,
                    None => return default,
                }
            }
            Screen::Help(_) => "Help".to_owned(),
            Screen::Exit { .. } => return default,
        };
        format!("{s} – Frigicom")
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ConfigReloaded(config) => {
                self.config_file_reloaded(config)
            }
            Message::RuntimeConfigured(result) => {
                if let Err(error) = result {
                    log::error!("failed to configure runtime: {error}");
                    Task::none()
                } else {
                    iced::system::information().map(Message::SystemInformation)
                }
            }
            Message::SystemInformation(information) => {
                if matches!(self.modal, Some(Modal::About(_))) {
                    Task::done(Message::Modal(modal::Message::About(
                        modal::about::Action::SystemInformation(information),
                    )))
                } else {
                    Task::none()
                }
            }
            Message::AppearanceReloaded(appearance) => {
                self.config.appearance = appearance;
                Task::none()
            }
            Message::ScreenConfigReloaded(updated) => {
                let saved_window = self.main_window;
                let current_runtime = self.config.runtime;
                let runtime =
                    updated.as_ref().ok().map(|config| config.runtime);

                let (mut frigicom, command) = Frigicom::load_from_state(
                    self.main_window.id,
                    updated,
                    self.current_mode,
                    self.guard,
                );
                frigicom.main_window = saved_window;
                // The backend session outlives a config reload: its
                // controller is delivered exactly once per stream and the
                // subscription is keyed independently of the config, so
                // dropping the handle here would sever the backend for the
                // rest of the run.
                frigicom.session = std::mem::take(&mut self.session);
                frigicom.backend = std::mem::take(&mut self.backend);
                frigicom.has_been_online = self.has_been_online;
                frigicom.modal_drafts = std::mem::take(&mut self.modal_drafts);
                *self = frigicom;
                self.sync_histories();

                let mut tasks = vec![command];

                // Only reload the runtime when needed.
                // TODO(upstream): can crash with NVIDIA Vulkan; the fix is in
                // wgpu — https://github.com/gfx-rs/wgpu/issues/9277
                if let Some(runtime) = runtime
                    && runtime != current_runtime
                {
                    tasks.push(configure_runtime(runtime));
                }

                Task::batch(tasks)
            }
            Message::Dashboard(message) => {
                let Screen::Dashboard(dashboard) = &mut self.screen else {
                    return Task::none();
                };

                let (command, event) = dashboard.update(
                    message,
                    &self.session,
                    &mut self.backend,
                    &mut self.theme,
                    &self.version,
                    &self.config,
                    &self.main_window,
                );

                // Retrack after dashboard state changes
                dashboard.track_histories(&mut self.backend);
                dashboard.track_member_loads(&self.session, &mut self.backend);

                let event_task = match event {
                    Some(dashboard::Event::ToggleFullscreen) => {
                        self.main_window.toggle_fullscreen();
                        self.save_main_window_settings()
                    }
                    Some(dashboard::Event::ConfigReloaded(config)) => {
                        self.config_file_reloaded(config)
                    }
                    Some(dashboard::Event::ReloadThemes) => {
                        Task::future(Config::load()).then(|config| match config
                        {
                            Ok(config) => Task::done(
                                Message::AppearanceReloaded(config.appearance),
                            ),
                            Err(_) => Task::none(),
                        })
                    }
                    Some(dashboard::Event::Exit) => self.begin_exit(),
                    Some(dashboard::Event::OpenUrl(
                        raw_url,
                        prompt_before_open,
                    )) => {
                        let Some((id, _, _)) = dashboard.get_focused() else {
                            return Task::none();
                        };

                        if prompt_before_open {
                            self.modal = Some(Modal::PromptBeforeOpenUrl {
                                url: raw_url,
                                window: id,
                            });
                        } else {
                            let canonical = ::url::Url::parse(&raw_url)
                                .map_or(raw_url, |u| u.to_string());
                            let _ = open_url::open(canonical);
                        }

                        Task::none()
                    }
                    Some(dashboard::Event::OpenAbout {
                        version,
                        commit,
                        system_information,
                    }) => {
                        self.modal =
                            Some(Modal::About(modal::about::About::new(
                                version,
                                commit,
                                system_information,
                                self.config.runtime,
                            )));

                        Task::none()
                    }
                    Some(dashboard::Event::OpenNewDm) => {
                        self.open_modal(Modal::NewDm(
                            modal::new_dm::NewDm::new(&self.modal_drafts.dm),
                        ))
                    }
                    Some(dashboard::Event::OpenNewGroup) => self.open_modal(
                        Modal::NewGroup(modal::new_group::NewGroup::new(
                            &self.modal_drafts.group_name,
                            &self.modal_drafts.group_description,
                        )),
                    ),
                    Some(dashboard::Event::OpenSetNickname(convo_id)) => {
                        match self.session.conversations.get(&convo_id) {
                            Some(conversation) => {
                                self.open_modal(Modal::SetNickname(
                                    modal::set_nickname::SetNickname::new(
                                        convo_id.clone(),
                                        conversation.display_name(),
                                        conversation.nickname.clone(),
                                    ),
                                ))
                            }
                            None => Task::none(),
                        }
                    }
                    Some(dashboard::Event::OpenAddMember(convo_id)) => self
                        .open_modal(Modal::AddMember(
                            modal::add_member::AddMember::new(
                                convo_id,
                                &self.modal_drafts.member,
                            ),
                        )),
                    Some(dashboard::Event::AddMemberRequested {
                        convo_id,
                        peer_address,
                    }) => self.add_group_member(convo_id, peer_address),
                    Some(dashboard::Event::ConfirmDeleteConversation(
                        convo_id,
                    )) => {
                        let display_name = self
                            .session
                            .conversations
                            .get(&convo_id)
                            .map_or_else(
                                || convo_id.short_label().to_string(),
                                data::Conversation::display_name,
                            );

                        self.open_modal(Modal::ConfirmDeleteConversation(
                            modal::confirm_delete::ConfirmDelete::new(
                                convo_id.clone(),
                                display_name,
                            ),
                        ))
                    }
                    None => Task::none(),
                };

                Task::batch(vec![event_task, command.map(Message::Dashboard)])
            }
            Message::Version(remote) => {
                // Set latest known remote version
                self.version.remote = remote;

                Task::none()
            }
            Message::Help(message) => {
                let Screen::Help(help) = &mut self.screen else {
                    return Task::none();
                };

                match help.update(message) {
                    Some(help::Event::RefreshConfiguration) => Task::perform(
                        Config::load(),
                        Message::ScreenConfigReloaded,
                    ),
                    None => Task::none(),
                }
            }
            Message::System(system::Event::Suspending) => {
                log::info!("system suspending");
                self.power = system::State::Suspended;
                Task::none()
            }
            Message::System(system::Event::Resumed) => {
                log::info!("system resumed");
                self.power = system::State::Awake;
                Task::none()
            }
            Message::Stream(update) => self.handle_backend_update(update),
            Message::Event(window, event) => {
                if let Screen::Dashboard(dashboard) = &mut self.screen {
                    return dashboard
                        .handle_event(
                            window,
                            event,
                            &self.session,
                            &self.version,
                            &self.config,
                            &mut self.theme,
                        )
                        .map(Message::Dashboard);
                }

                Task::none()
            }
            Message::Tick(now) => {
                if let Screen::Exit { deadline } = &self.screen
                    && now >= *deadline
                {
                    log::warn!(
                        "backend did not acknowledge shutdown within \
                         {EXIT_DEADLINE:?}; exiting anyway"
                    );

                    return iced::exit();
                }

                if let Screen::Dashboard(dashboard) = &mut self.screen {
                    dashboard.tick(now).map(Message::Dashboard)
                } else {
                    Task::none()
                }
            }
            Message::Modal(message) => {
                let Some(modal) = &mut self.modal else {
                    return Task::none();
                };

                let (command, event) = modal.update(message);

                if let Some(event) = event {
                    match event {
                        modal::Event::AddGroupMember {
                            convo_id,
                            peer_address,
                        } => {
                            self.modal = None;
                            self.modal_drafts.member.clear();

                            return self
                                .add_group_member(convo_id, peer_address);
                        }
                        modal::Event::ConfirmAddGroupMember {
                            convo_id,
                            peer_address,
                            dont_show_again,
                        } => {
                            self.modal = None;

                            if dont_show_again
                                && let Screen::Dashboard(dashboard) =
                                    &mut self.screen
                            {
                                dashboard.set_member_add_explained();
                            }

                            self.backend.send(
                                stream::Control::AddGroupMember {
                                    convo_id: dashboard::wire_id(&convo_id),
                                    peer_address,
                                },
                            );
                        }
                        modal::Event::CloseModal => {
                            if let Some(modal) = self.modal.take() {
                                self.stash_modal_draft(&modal);
                            }
                        }
                        modal::Event::CreateDirectMessage { peer_address } => {
                            self.modal = None;
                            self.modal_drafts.dm.clear();
                            self.backend.send(
                                stream::Control::CreateConversation {
                                    peer_address,
                                },
                            );
                        }
                        modal::Event::CreateGroup { name, description } => {
                            self.modal = None;
                            self.modal_drafts.group_name.clear();
                            self.modal_drafts.group_description.clear();
                            self.backend.send(stream::Control::CreateGroup {
                                name,
                                description,
                            });
                        }
                        modal::Event::SetNickname { convo_id, nickname } => {
                            self.modal = None;
                            self.backend.send(stream::Control::SetNickname {
                                convo_id: dashboard::wire_id(&convo_id),
                                nickname,
                            });
                        }
                        modal::Event::DeleteConversation(convo_id) => {
                            self.modal = None;
                            self.backend.send(
                                stream::Control::DeleteConversation {
                                    convo_id: dashboard::wire_id(&convo_id),
                                },
                            );
                        }
                    }
                }

                command.map(Message::Modal)
            }
            Message::RouteReceived(route) => {
                log::info!("RouteReceived: {route:?}");

                // A later instance with nothing to route just wants us
                // in front of it.
                if route == ipc::FOCUS {
                    return window::gain_focus(self.main_window.id);
                }

                if let Ok(url) = route.parse() {
                    return self.handle_url(url);
                };

                Task::none()
            }
            Message::Window(id, event) => {
                let mut tasks = vec![];

                match &event {
                    window::Event::Focused => {
                        if self.focused_window != Some(id) {
                            tasks.push(iced::window::request_user_attention(
                                id, None,
                            ));
                        }

                        self.focused_window = Some(id);
                    }
                    window::Event::Unfocused => {
                        if self.focused_window == Some(id) {
                            self.focused_window = None;
                        }
                    }
                    window::Event::Opened { .. }
                    | window::Event::Moved(_)
                    | window::Event::Resized(_)
                    | window::Event::CloseRequested
                    | window::Event::FileHovered
                    | window::Event::FilesHoveredLeft
                    | window::Event::FileDropped(_) => {}
                }

                if id == self.main_window.id {
                    match event {
                        window::Event::Moved(position) => {
                            self.main_window.position = Some(position);
                        }
                        window::Event::Resized(size) => {
                            self.main_window.size = size;
                        }
                        window::Event::Focused => {
                            self.main_window.focused = true;
                        }
                        window::Event::Unfocused => {
                            self.main_window.focused = false;
                        }
                        window::Event::Opened { position, size } => {
                            self.main_window.opened(position, size);
                        }
                        window::Event::CloseRequested => {
                            let save = self.save_main_window_settings();

                            if let Screen::Dashboard(dashboard) =
                                &mut self.screen
                            {
                                return save.chain(
                                    dashboard
                                        .exit(&self.config)
                                        .map(Message::Dashboard),
                                );
                            } else {
                                return save.chain(iced::exit());
                            }
                        }
                        window::Event::FileHovered
                        | window::Event::FilesHoveredLeft
                        | window::Event::FileDropped(_) => {}
                    }

                    tasks.push(
                        iced::window::is_maximized(self.main_window.id)
                            .map(Message::WindowMaximizeChecked),
                    );

                    if let Some(Screen::Dashboard(dashboard)) =
                        matches!(event, window::Event::Focused)
                            .then_some(&mut self.screen)
                    {
                        tasks.push(
                            dashboard
                                .focus_window_pane(self.main_window.id)
                                .map(Message::Dashboard),
                        );
                    }
                } else if let Screen::Dashboard(dashboard) = &mut self.screen {
                    tasks.push(
                        dashboard
                            .handle_window_event(id, event, &mut self.theme)
                            .map(Message::Dashboard),
                    );
                }

                Task::batch(tasks)
            }
            Message::WindowMaximizeChecked(is_maximized) => {
                self.main_window.update_maximize(is_maximized);
                self.save_main_window_settings()
            }
            Message::WindowSettingsSaved(result) => {
                if let Err(err) = result {
                    log::error!("window settings failed to save: {err:?}");
                }

                Task::none()
            }
            Message::AppearanceChange(mode) => {
                if let data::appearance::Selected::Dynamic { .. } =
                    &self.config.appearance.selected
                {
                    self.current_mode = mode;
                    self.theme = self
                        .current_mode
                        .theme(&self.config.appearance.selected)
                        .into();
                }

                Task::none()
            }
            Message::Logging(mut records) => {
                let Screen::Dashboard(dashboard) = &mut self.screen else {
                    self.pending_logs.extend(records);

                    return Task::none();
                };

                // We've moved from non-dashboard screen to dashboard, prepend records
                if !self.pending_logs.is_empty() {
                    records = std::mem::take(&mut self.pending_logs)
                        .into_iter()
                        .chain(records)
                        .collect();
                }

                for record in records.into_iter().filter(|record| {
                    record.level <= self.config.logs.pane_level
                }) {
                    dashboard.record_log(record);
                }

                Task::none()
            }
            Message::ModuleLogging(lines) => {
                for line in lines {
                    // Ahead of the level filter on purpose. A crash can be
                    // announced by a line whose own dialect calls it `info`
                    // (`logos-modules.md` §5), and losing the fact because of a
                    // display setting would leave the row looking idle.
                    if line.record.reports_crash() {
                        self.session.note_module_crash(&line.module);
                    }

                    if !self.config.modules.admits(line.level()) {
                        continue;
                    }

                    // Nothing is buffered for a screen that is not the
                    // dashboard: unlike the app's own log this is a tail of a
                    // file that is still on disk, and the only screens that
                    // are not the dashboard are the config-error screen and
                    // the one shown while quitting.
                    if let Screen::Dashboard(dashboard) = &mut self.screen {
                        dashboard.record_module_log(line);
                    }
                }

                Task::none()
            }
            Message::UnixSignal(signal) => match signal {
                #[cfg(target_family = "unix")]
                signal_hook::consts::SIGUSR1 => {
                    Task::perform(Config::load(), Message::ConfigReloaded)
                }
                #[cfg(target_family = "unix")]
                signal_hook::consts::SIGTERM | signal_hook::consts::SIGINT => {
                    if let Screen::Dashboard(dashboard) = &mut self.screen {
                        dashboard.exit(&self.config).map(Message::Dashboard)
                    } else {
                        iced::exit()
                    }
                }
                _ => Task::none(),
            },
            Message::Notification(event) => {
                if let Screen::Dashboard(dashboard) = &mut self.screen {
                    dashboard
                        .handle_notification_event(
                            event,
                            &mut self.backend,
                            &self.config,
                        )
                        .map(Message::Dashboard)
                } else {
                    Task::none()
                }
            }
        }
    }

    /// Reconciles the panes against the live conversation set, then ensures
    /// a history entry per known conversation (so unread state accrues even
    /// without an open pane) and retracks pane histories against the live
    /// backend, requesting message loads for open conversation panes whose
    /// history is not `Full` yet. Restored panes are otherwise stuck empty:
    /// `Dashboard::restore` runs before the controller exists, so its
    /// initial load requests are dropped.
    ///
    /// Every path that restores panes from disk funnels through here — the
    /// first `ConversationsSnapshot` after launch and the screen rebuild a
    /// config reload performs — so this is where the invariant is kept:
    /// after either one, no pane names a conversation the backend does not
    /// have, and nothing is asked about a dead id. Reconciling first is
    /// what makes the second half true; the tracking below would otherwise
    /// open a history and request messages for the dead id it just found.
    fn sync_histories(&mut self) {
        if let Screen::Dashboard(dashboard) = &mut self.screen {
            dashboard.reconcile_conversations(&self.session);

            for conversation in self.session.conversations.iter() {
                dashboard.open_history(history::Kind::Conversation(
                    conversation.id.clone(),
                ));
            }

            dashboard.track_histories(&mut self.backend);
            dashboard.track_member_loads(&self.session, &mut self.backend);
        }
    }

    /// Folds one backend update into domain state, then wires the history
    /// and dashboard side effects.
    fn handle_backend_update(&mut self, update: Update) -> Task<Message> {
        let was_online = self.session.delivery.can_act();

        self.session.apply(&update);

        self.notify_connectivity(was_online);

        match update {
            Update::Controller(controller) => {
                self.backend.set_controller(controller);
            }
            Update::Phase(phase) => {
                log::debug!("backend phase: {phase:?}");

                // Non-Online phases fold into `session.delivery`, which
                // the status bar spells out; only Failed is queued as an
                // error (the Fatal update right after carries the detail).
                if matches!(phase, Phase::Failed) {
                    log::error!("backend session failed");
                }
            }
            Update::Ready {
                my_address,
                installation_name,
            } => {
                log::info!(
                    "backend ready: address {} (installation {:?})",
                    data::address::short_label(&my_address),
                    installation_name,
                );
            }
            Update::ConversationsSnapshot(conversations) => {
                if let Screen::Dashboard(dashboard) = &mut self.screen {
                    dashboard.reset_member_requests();
                }

                // The snapshot has already replaced the conversation map
                // (`Session::apply`, above), so the reconciliation
                // `sync_histories` opens with sees the authoritative set —
                // and runs before any load is requested below.
                self.sync_histories();

                // Snapshots carry no member lists and the only other
                // producer is a `members_changed` push event, so DMs would
                // otherwise never learn their peer (QML refetches members
                // on every select and again on the fresh-online resync —
                // both paths land here).
                for conversation in conversations {
                    self.backend.send(stream::Control::LoadMembers {
                        convo_id: conversation.convo_id,
                    });
                }
            }
            Update::MessagesLoaded { convo_id, messages } => {
                if let Screen::Dashboard(dashboard) = &mut self.screen {
                    let id = ConvoId::from(&convo_id);
                    let messages = messages
                        .into_iter()
                        .map(|message| wire_message(&id, message))
                        .collect();

                    return dashboard
                        .load_messages(
                            history::Kind::Conversation(id),
                            messages,
                        )
                        .map(Message::Dashboard);
                }
            }
            Update::MembersLoaded { .. } => {}
            // Already folded into `self.session` above, which is all the
            // sidebar rows and the pane's status strip read. Nothing else
            // has to happen when the module set moves.
            Update::Modules(_) => {}
            Update::Event(event) => return self.handle_chat_event(event),
            Update::ActionFailed(error) => match error {
                ActionError::SendFailed {
                    convo_id,
                    content,
                    reason,
                } => {
                    log::warn!("send failed: {reason}");

                    if let Screen::Dashboard(dashboard) = &mut self.screen {
                        dashboard
                            .restore_draft(&ConvoId::from(&convo_id), &content);
                        dashboard
                            .report_error(format!("Failed to send: {reason}"));
                    }
                }
                error => {
                    log::warn!("backend action failed: {error}");

                    if let Screen::Dashboard(dashboard) = &mut self.screen {
                        dashboard.report_error(error.to_string());
                    }
                }
            },
            Update::Fatal(error) => {
                log::error!("backend session is dead: {error}");

                // The session ends on a fatal error; a controller kept past
                // it would make `Map::quit` report a shutdown to wait for
                // that nothing will ever acknowledge.
                self.backend.clear_controller();

                if matches!(self.screen, Screen::Exit { .. }) {
                    return iced::exit();
                }

                if let Screen::Dashboard(dashboard) = &mut self.screen {
                    dashboard.report_error(fatal_error_message(&error));
                }
            }
            Update::Stopped => {
                self.backend.clear_controller();

                if matches!(self.screen, Screen::Exit { .. }) {
                    return iced::exit();
                }
            }
        }

        Task::none()
    }

    /// Emits Connected/Reconnected/Disconnected notifications on delivery
    /// transitions (both `Phase` changes and `delivery_state_changed`
    /// events fold into `session.delivery` before this runs).
    fn notify_connectivity(&mut self, was_online: bool) {
        let is_online = self.session.delivery.can_act();

        if matches!(self.screen, Screen::Exit { .. })
            || self.power.suppresses_connection_events()
            || was_online == is_online
        {
            return;
        }

        let notification = if is_online {
            if self.has_been_online {
                Notification::Reconnected
            } else {
                self.has_been_online = true;
                Notification::Connected
            }
        } else {
            Notification::Disconnected
        };

        self.notifications.notify(&self.config, &notification);
    }

    fn handle_chat_event(&mut self, event: ChatEvent) -> Task<Message> {
        let Screen::Dashboard(dashboard) = &mut self.screen else {
            return Task::none();
        };

        match event {
            ChatEvent::MessageReceived {
                convo_id,
                content,
                timestamp_ms,
                sender,
            } => {
                let id = ConvoId::from(&convo_id);
                let sender = (!sender.is_empty())
                    .then(|| Address::from(sender.as_str()));

                let message = data::Message::received(
                    id.clone(),
                    sender.clone(),
                    content.clone(),
                    timestamp_ms,
                );

                let kind = history::Kind::Conversation(id.clone());
                let message_window = dashboard.find_window_with_history(&kind);
                dashboard.record_message(kind, message);

                if message_window.is_none() || !self.main_window.focused {
                    let conversation = self.session.conversations.get(&id);
                    let notification = match conversation
                        .map(|conversation| conversation.kind)
                    {
                        Some(ConversationKind::Group) => {
                            Notification::GroupMessage {
                                convo_id: id,
                                title: conversation
                                    .map(data::Conversation::display_name)
                                    .unwrap_or_default(),
                                message: content,
                            }
                        }
                        _ => Notification::DirectMessage {
                            convo_id: id,
                            sender: sender.unwrap_or_else(|| {
                                Address::from(data::message::UNKNOWN_SENDER)
                            }),
                            message: content,
                        },
                    };

                    self.notifications.notify(&self.config, &notification);
                }
            }
            ChatEvent::MessageSent {
                convo_id,
                content,
                timestamp_ms,
            } => {
                let id = ConvoId::from(&convo_id);

                dashboard.record_message(
                    history::Kind::Conversation(id.clone()),
                    data::Message::sent(id, content, timestamp_ms),
                );
            }
            ChatEvent::ConversationCreated {
                convo_id,
                is_outgoing,
                kind,
                name,
                ..
            } => {
                // A fresh DM emits no `members_changed`, so fetch the
                // roster now or `peer_address()` stays unknown forever
                // (QML refetches members on selecting the conversation).
                self.backend.send(stream::Control::LoadMembers {
                    convo_id: convo_id.clone(),
                });

                let id = ConvoId::from(&convo_id);
                let history_kind = history::Kind::Conversation(id.clone());

                dashboard.open_history(history_kind.clone());

                if is_outgoing {
                    dashboard.record_message(
                        history_kind,
                        data::Message::status(
                            id.clone(),
                            data::message::StatusKind::NewConversation,
                            "conversation created".to_string(),
                        ),
                    );

                    // A conversation this account created becomes the
                    // focused pane (QML selects it on creation).
                    return dashboard
                        .open_conversation(
                            id,
                            BufferAction::ReplacePane,
                            &mut self.backend,
                            &self.config,
                        )
                        .map(Message::Dashboard);
                } else {
                    // No message flows for an incoming invite; bump the
                    // unread state explicitly.
                    dashboard.record_message(
                        history_kind.clone(),
                        data::Message::status(
                            id.clone(),
                            data::message::StatusKind::NewConversation,
                            "you were added to this conversation".to_string(),
                        ),
                    );
                    dashboard.mark_unread(&history_kind);

                    if matches!(
                        data::conversation::Kind::from(kind),
                        ConversationKind::Group
                    ) {
                        self.notifications.notify(
                            &self.config,
                            &Notification::GroupInvite {
                                convo_id: id.clone(),
                                title: if name.is_empty() {
                                    format!("Group {}", id.short_label())
                                } else {
                                    name
                                },
                            },
                        );
                    }
                }
            }
            ChatEvent::ConversationDeleted { convo_id } => {
                dashboard.conversation_deleted(&ConvoId::from(&convo_id));
            }
            // The session machine refetches on these; state arrives via
            // `ConversationsSnapshot` / `MembersLoaded`.
            ChatEvent::ConversationUpdated { .. }
            | ChatEvent::MembersChanged { .. } => {}
            // Folded by `Session::apply`; connectivity notifications are
            // wired with the status bar in M5.
            ChatEvent::DeliveryStateChanged { .. } => {}
        }

        Task::none()
    }

    /// Presents a modal and focuses its primary input.
    fn open_modal(&mut self, modal: Modal) -> Task<Message> {
        let focus = modal.focus().map(Message::Modal);
        self.modal = Some(modal);
        focus
    }

    /// Sends a group invite, routing the first one through the
    /// "commits take up to a minute" explainer until it has been
    /// dismissed with don't-show-again (QML parity).
    fn add_group_member(
        &mut self,
        convo_id: ConvoId,
        peer_address: String,
    ) -> Task<Message> {
        let explained = match &self.screen {
            Screen::Dashboard(dashboard) => dashboard.member_add_explained(),
            Screen::Help(_) | Screen::Exit { .. } => true,
        };

        if explained {
            self.modal = None;
            self.backend.send(stream::Control::AddGroupMember {
                convo_id: dashboard::wire_id(&convo_id),
                peer_address,
            });

            Task::none()
        } else {
            self.open_modal(Modal::MemberAddInfo(
                modal::member_add_info::MemberAddInfo::new(
                    convo_id,
                    peer_address,
                ),
            ))
        }
    }

    /// Keeps a dismissed dialog's entries so it reopens with the same
    /// draft (QML parity); drafts clear on a successful create.
    fn stash_modal_draft(&mut self, modal: &Modal) {
        match modal {
            Modal::NewDm(new_dm) => {
                self.modal_drafts.dm = new_dm.draft();
            }
            Modal::NewGroup(new_group) => {
                let (name, description) = new_group.draft();
                self.modal_drafts.group_name = name;
                self.modal_drafts.group_description = description;
            }
            Modal::AddMember(add_member) => {
                self.modal_drafts.member = add_member.draft();
            }
            Modal::ReloadConfigurationError(..)
            | Modal::About(..)
            | Modal::PromptBeforeOpenUrl { .. }
            | Modal::SetNickname(..)
            | Modal::ConfirmDeleteConversation(..)
            | Modal::MemberAddInfo(..) => {}
        }
    }

    /// Requests a clean backend shutdown; the app exits when
    /// `Update::Stopped` (or a fatal error) arrives.
    fn begin_exit(&mut self) -> Task<Message> {
        if self.backend.quit() {
            self.screen = Screen::Exit {
                deadline: Instant::now() + EXIT_DEADLINE,
            };
            Task::none()
        } else {
            iced::exit()
        }
    }

    fn view(&self, id: window::Id) -> Element<'_, Message> {
        let platform_specific_padding =
            platform_specific::content_padding(&self.config);

        // Main window.
        if id == self.main_window.id {
            let screen = match &self.screen {
                Screen::Dashboard(dashboard) => dashboard
                    .view(
                        &self.session,
                        &self.version,
                        &self.config,
                        &self.theme,
                    )
                    .map(Message::Dashboard),
                Screen::Help(help) => help.view(&self.theme).map(Message::Help),
                Screen::Exit { .. } => container(
                    text("Disconnecting…").style(theme::text::secondary),
                )
                .center(Length::Fill)
                .into(),
            };

            let content = container(
                container(screen)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(theme::container::root),
            )
            .padding(padding::top(platform_specific_padding));

            // Modals might have a id representing which window to be presented on.
            // If modal has no id, we show them on main_window.
            match &self.modal {
                Some(modal)
                    if modal.window_id() == Some(self.main_window.id)
                        || modal.window_id().is_none() =>
                {
                    widget::modal(
                        content,
                        modal.view(&self.theme).map(Message::Modal),
                        || Message::Modal(modal::Message::Cancel),
                        0.8,
                    )
                }
                _ => column![content].into(),
            }
        // Popped out window.
        } else if let Screen::Dashboard(dashboard) = &self.screen {
            let content = container(
                dashboard
                    .view_window(
                        id,
                        &self.session,
                        &self.version,
                        &self.config,
                        &self.theme,
                    )
                    .map(Message::Dashboard),
            )
            .padding(padding::top(platform_specific_padding));

            // Modals might have a id representing which window to be presented on.
            // If modal id match the current id we show it.
            match &self.modal {
                Some(modal) if modal.window_id() == Some(id) => widget::modal(
                    content,
                    modal.view(&self.theme).map(Message::Modal),
                    || Message::Modal(modal::Message::Cancel),
                    0.8,
                ),
                _ => column![content].into(),
            }
        } else {
            column![].into()
        }
    }

    fn theme(&self, _window: window::Id) -> Theme {
        self.theme.clone()
    }

    fn scale_factor(&self, _window: window::Id) -> f32 {
        f32::from(self.config.scale_factor)
    }

    fn subscription(&self) -> Subscription<Message> {
        let tick = iced::time::every(Duration::from_secs(1)).map(Message::Tick);

        // The one backend session; a singleton key, so it survives config
        // reloads untouched.
        let backend =
            stream::subscription(&self.config, self.guard).map(Message::Stream);

        // Deliberately not part of the backend subscription: tailing a file
        // needs no daemon handshake, so the log keeps filling while the
        // session is down — which is exactly when it is worth reading.
        let module_logs =
            module_log::subscription(&self.config).map(Message::ModuleLogging);

        let mut subscriptions = vec![
            url::listen().map(Message::RouteReceived),
            events().map(|(window, event)| Message::Event(window, event)),
            window::events()
                .map(|(window, event)| Message::Window(window, event)),
            system::events().map(Message::System),
            tick,
            backend,
            module_logs,
        ];

        if cfg!(target_family = "unix") {
            subscriptions
                .push(unix_signal::subscription().map(Message::UnixSignal));
        }

        // We only want to listen for appearance changes if user has dynamic themes.
        if self.config.appearance.selected.is_dynamic() {
            subscriptions.push(
                appearance::subscription().map(Message::AppearanceChange),
            );
        }

        Subscription::batch(subscriptions)
    }

    fn config_file_reloaded(
        &mut self,
        config: Result<Config, config::Error>,
    ) -> Task<Message> {
        match config {
            Ok(updated) => {
                // Only reload the runtime when needed.
                // TODO(upstream): can crash with NVIDIA Vulkan; the fix is in
                // wgpu — https://github.com/gfx-rs/wgpu/issues/9277
                let runtime_task = (self.config.runtime != updated.runtime)
                    .then(|| configure_runtime(updated.runtime));

                self.theme = self
                    .current_mode
                    .theme(&updated.appearance.selected)
                    .into();

                // Load new notification sounds.
                self.notifications.update(&updated);

                self.config = updated;

                return runtime_task.unwrap_or_else(Task::none);
            }
            Err(error) => {
                self.modal = Some(Modal::ReloadConfigurationError(error));
            }
        }

        Task::none()
    }
}

/// Spells a dead session out for the status bar. The two failures a user
/// can actually fix — no logoscore artifacts, or a build without the live
/// transport — carry the remedy alongside the error.
fn fatal_error_message(error: &stream::BackendError) -> String {
    match error {
        stream::BackendError::FfiUnavailable => format!(
            "Backend failed: {error}. Rebuild with `cargo run --features \
             live` (inside `. scripts/dev-env.sh`), or set `mock = true` \
             under [logos] in config.toml to use the offline mock."
        ),
        stream::BackendError::ArtifactsMissing { .. } => format!(
            "Backend failed: {error}. Set logoscore paths in config.toml \
             [logos] (daemon_path, modules_dir) or run inside the dev shell."
        ),
        _ => format!("Backend failed: {error}"),
    }
}

/// Converts a wire message from a `get_messages` snapshot into a domain
/// message.
fn wire_message(convo_id: &ConvoId, message: stream::Message) -> data::Message {
    if message.from_self {
        data::Message::sent(
            convo_id.clone(),
            message.content,
            message.timestamp_ms,
        )
    } else {
        data::Message::received(
            convo_id.clone(),
            message
                .sender
                .filter(|sender| !sender.is_empty())
                .map(Address::from),
            message.content,
            message.timestamp_ms,
        )
    }
}
