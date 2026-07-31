use std::collections::{HashMap, HashSet, VecDeque};
use std::convert;
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

use data::conversation::{ConvoId, Kind as ConversationKind};
use data::dashboard::{self, BufferAction};
use data::environment::RELEASE_WEBSITE;
use data::input::RawInput;
use data::{Config, Version, config, environment, history, stream};
use iced::widget::pane_grid::{self, PaneGrid};
use iced::widget::{Space, column, container, row, stack};
use iced::{Length, Size, Task, Vector, clipboard, padding};

use self::command_bar::CommandBar;
use self::pane::Pane;
use self::sidebar::Sidebar;
use self::theme_editor::ThemeEditor;
use crate::buffer::{self, Buffer};
use crate::notification::{self, toast};
use crate::widget::{
    Column, Element, Row, anchored_overlay, context_menu, selectable_text,
    shortcut,
};
use crate::window::Window;
use crate::{Theme, event, open_url, platform_specific, theme, window};

pub mod account_card;
pub mod command_bar;
pub mod pane;
pub mod sidebar;
pub mod status_bar;
mod theme_editor;

const FOCUS_HISTORY_LEN: usize = 8;
const SAVE_AFTER: Duration = Duration::from_secs(3);

pub struct Dashboard {
    panes: Panes,
    focus: Focus,
    focus_history: VecDeque<pane_grid::Pane>,
    side_menu: Sidebar,
    history: history::Manager,
    last_changed: Option<Instant>,
    command_bar: Option<CommandBar>,
    command_bar_window: Option<window::Id>,
    theme_editor: Option<ThemeEditor>,
    buffer_settings: dashboard::BufferSettings,
    status_bar: status_bar::StatusBar,
    /// Whether the first-add explainer has been dismissed with
    /// don't-show-again (persisted with the dashboard).
    member_add_explained: bool,
    /// Conversations whose member list has been requested this snapshot
    /// generation — dedupes the once-per-open fallback fetch.
    requested_member_loads: HashSet<ConvoId>,
}

#[derive(Debug)]
pub enum Message {
    Pane(window::Id, pane::Message),
    Sidebar(sidebar::Message),
    SelectedText(Vec<(RangeInclusive<f32>, String)>, clipboard::ClipboardKind),
    DashboardSaved(Result<(), data::dashboard::Error>),
    Exited(Result<(), data::dashboard::Error>),
    Task(command_bar::Message),
    Shortcut(shortcut::Command),
    CloseContextMenu(window::Id, bool),
    ThemeEditor(theme_editor::Message),
    ConfigReloaded(Result<Config, config::Error>),
    ConfigEditorReloaded(Result<Config, config::Error>),
    NewWindow(window::Id, Pane),
    StatusBar(status_bar::Message),
}

#[derive(Debug)]
pub enum Event {
    ConfigReloaded(Result<Config, config::Error>),
    ReloadThemes,
    Exit,
    OpenUrl(String, bool),
    OpenAbout {
        version: String,
        commit: String,
        system_information: Option<iced::system::Information>,
    },
    ToggleFullscreen,
    OpenNewDm,
    OpenNewGroup,
    OpenSetNickname(ConvoId),
    ConfirmDeleteConversation(ConvoId),
    /// Open the add-member dialog to collect an address for this group.
    OpenAddMember(ConvoId),
    /// An invite with a known address (roster dialog or `/add`); the app
    /// runs the first-add explainer gate before sending.
    AddMemberRequested {
        convo_id: ConvoId,
        peer_address: String,
    },
}

impl Dashboard {
    pub fn empty(
        main_window: &Window,
        config: &Config,
    ) -> (Self, Task<Message>) {
        let (main_panes, pane) =
            pane_grid::State::new(Pane::new(Buffer::Empty));

        let (sidebar, sidebar_task) = Sidebar::new(false);

        let mut dashboard = Dashboard {
            panes: Panes {
                main_window: main_window.id,
                main: main_panes,
                popout: HashMap::new(),
            },
            focus: Focus {
                window: main_window.id,
                pane,
            },
            focus_history: VecDeque::new(),
            side_menu: sidebar,
            history: history::Manager::default(),
            last_changed: None,
            command_bar: None,
            command_bar_window: None,
            theme_editor: None,
            buffer_settings: dashboard::BufferSettings::default(),
            status_bar: status_bar::StatusBar::default(),
            member_add_explained: false,
            requested_member_loads: HashSet::new(),
        };

        let _ = config;
        dashboard.track_histories(&mut stream::Map::default());

        (dashboard, sidebar_task.map(Message::Sidebar))
    }

    pub fn restore(
        mut dashboard: data::Dashboard,
        config: &Config,
        main_window: &Window,
    ) -> (Self, Task<Message>) {
        if !config.pane.restore_on_launch {
            dashboard.pane = data::Pane::Empty;
            dashboard.popout_panes.clear();
            dashboard.focus_buffer = None;
        }

        let (mut dashboard, task) =
            Dashboard::from_data(dashboard, config, main_window);

        dashboard.track_histories(&mut stream::Map::default());

        (dashboard, task)
    }

    pub fn update(
        &mut self,
        message: Message,
        session: &data::Session,
        backend: &mut stream::Map,
        theme: &mut Theme,
        version: &Version,
        config: &Config,
        main_window: &Window,
    ) -> (Task<Message>, Option<Event>) {
        match message {
            Message::Pane(window, message) => {
                match message {
                    pane::Message::PaneClicked(pane) => {
                        return (self.focus_pane(window, pane), None);
                    }
                    pane::Message::PaneResized(pane_grid::ResizeEvent {
                        split,
                        ratio,
                    }) => {
                        // Pane grid interactions only enabled for main window panegrid
                        self.panes.main.resize(split, ratio);
                        self.last_changed = Some(Instant::now());
                    }
                    pane::Message::PaneDragged(
                        pane_grid::DragEvent::Dropped { pane, target },
                    ) => {
                        // Pane grid interactions only enabled for main window panegrid
                        self.panes.main.drop(pane, target);
                        self.last_changed = Some(Instant::now());
                    }
                    pane::Message::PaneDragged(_) => {}
                    pane::Message::ClosePane => {
                        return (
                            self.close_pane(
                                config,
                                self.focus.window,
                                self.focus.pane,
                            ),
                            None,
                        );
                    }
                    pane::Message::SplitPane(axis) => {
                        return (self.split_pane(axis), None);
                    }
                    pane::Message::Buffer(id, message) => {
                        if let Some(pane) = self.panes.get_mut(window, id) {
                            let (command, event) = pane.buffer.update(
                                message,
                                &mut self.history,
                                config,
                            );

                            let task = command.map(move |message| {
                                Message::Pane(
                                    window,
                                    pane::Message::Buffer(id, message),
                                )
                            });

                            let Some(event) = event else {
                                return (task, None);
                            };

                            let (buffer_task, buffer_event) = self
                                .handle_buffer_event(
                                    window, id, event, session, backend, config,
                                );

                            return (
                                Task::batch(vec![task, buffer_task]),
                                buffer_event,
                            );
                        }
                    }
                    pane::Message::ToggleShowMemberList => {
                        if let Some((_, _, pane)) = self.get_focused_mut() {
                            if let Some(buffer) = pane.buffer.data() {
                                let settings = self.buffer_settings.entry(
                                    &buffer,
                                    Some(config.buffer.clone().into()),
                                );
                                settings
                                    .conversation
                                    .member_list
                                    .toggle_visibility();
                            }

                            self.last_changed = Some(Instant::now());
                            return (Task::none(), None);
                        }
                    }
                    pane::Message::ToggleShowDetails => {
                        self.toggle_details_on_focused();
                        return (Task::none(), None);
                    }
                    pane::Message::MaximizePane => self.maximize_pane(),
                    pane::Message::Popout => {
                        return (self.popout_pane(config), None);
                    }
                    pane::Message::Merge => {
                        return (self.merge_pane(config), None);
                    }
                    pane::Message::ScrollToBottom => {
                        let Focus { window, pane } = self.focus;

                        if let Some(state) = self.panes.get_mut(window, pane) {
                            let mut task = state
                                .buffer
                                .scroll_to_end(config)
                                .map(move |message| {
                                    Message::Pane(
                                        window,
                                        pane::Message::Buffer(pane, message),
                                    )
                                });

                            if config.buffer.mark_as_read.on_scroll_to_bottom {
                                task = task.chain(Task::done(Message::Pane(
                                    window,
                                    pane::Message::MarkAsRead,
                                )));
                            }

                            return (task, None);
                        }
                    }
                    pane::Message::MarkAsRead => {
                        if let Some((_, _, pane)) = self.get_focused_mut()
                            && let Some(kind) = pane
                                .buffer
                                .data()
                                .and_then(history::Kind::from_buffer)
                        {
                            self.history.mark_as_read(&kind);
                        }
                    }
                    pane::Message::ClearBuffer => {
                        if let Some((_, _, pane)) = self.get_focused_mut()
                            && let Some(kind) = pane
                                .buffer
                                .data()
                                .and_then(history::Kind::from_buffer)
                        {
                            self.history.clear_messages(&kind);
                        }
                    }
                    pane::Message::ContentResized(id, size) => {
                        if let Some(state) = self.panes.get_mut(window, id) {
                            state.size = size;
                            state.buffer.update_pane_size(size, config);
                        }
                    }
                }
            }
            Message::Sidebar(message) => {
                let (command, event) = self.side_menu.update(message);

                let Some(event) = event else {
                    return (command.map(Message::Sidebar), None);
                };

                let (event_task, event) = match event {
                    sidebar::Event::QuitApplication => {
                        (self.exit(config), None)
                    }
                    sidebar::Event::New(buffer) => (
                        self.open_buffer(
                            buffer,
                            BufferAction::NewPane,
                            backend,
                            config,
                        ),
                        None,
                    ),
                    sidebar::Event::Popout(buffer) => (
                        self.open_buffer(
                            buffer,
                            BufferAction::NewWindow,
                            backend,
                            config,
                        ),
                        None,
                    ),
                    sidebar::Event::Focus(window, pane) => {
                        (self.focus_pane(window, pane), None)
                    }
                    sidebar::Event::Replace(buffer) => (
                        self.open_buffer(
                            buffer,
                            BufferAction::ReplacePane,
                            backend,
                            config,
                        ),
                        None,
                    ),
                    sidebar::Event::Close(window, pane) => {
                        (self.close_pane(config, window, pane), None)
                    }
                    sidebar::Event::Swap(window, pane) => {
                        (self.swap_pane_with_focus(window, pane), None)
                    }
                    sidebar::Event::ToggleCommandBar => (
                        self.toggle_command_bar(
                            session, version, config, theme,
                        ),
                        None,
                    ),
                    sidebar::Event::ConfigReloaded(conf) => {
                        (Task::none(), Some(Event::ConfigReloaded(conf)))
                    }
                    sidebar::Event::OpenReleaseWebsite => {
                        let _ = open_url::open(RELEASE_WEBSITE);
                        (Task::none(), None)
                    }
                    sidebar::Event::ToggleThemeEditor => (
                        self.toggle_theme_editor(theme, main_window, config),
                        None,
                    ),
                    sidebar::Event::OpenAbout {
                        version,
                        commit,
                        system_information,
                    } => (
                        Task::none(),
                        Some(Event::OpenAbout {
                            version,
                            commit,
                            system_information,
                        }),
                    ),
                    sidebar::Event::MarkAsRead(buffer) => {
                        if let Some(kind) = history::Kind::from_buffer(buffer) {
                            self.history.mark_as_read(&kind);
                        }

                        (Task::none(), None)
                    }
                    sidebar::Event::OpenNewDm => {
                        (Task::none(), Some(Event::OpenNewDm))
                    }
                    sidebar::Event::OpenNewGroup => {
                        (Task::none(), Some(Event::OpenNewGroup))
                    }
                    sidebar::Event::OpenSetNickname(convo_id) => {
                        (Task::none(), Some(Event::OpenSetNickname(convo_id)))
                    }
                    sidebar::Event::ConfirmDeleteConversation(convo_id) => (
                        Task::none(),
                        Some(Event::ConfirmDeleteConversation(convo_id)),
                    ),
                };

                let window = main_window.id;

                return (
                    Task::batch(vec![
                        context_menu::close(convert::identity).map(
                            move |any_closed| {
                                Message::CloseContextMenu(window, any_closed)
                            },
                        ),
                        event_task,
                        command.map(Message::Sidebar),
                    ]),
                    event,
                );
            }
            Message::SelectedText(contents, clipboard_kind) => {
                let mut last_y_range: Option<RangeInclusive<f32>> = None;
                let contents = contents.into_iter().fold(
                    String::new(),
                    |acc, (y_range, content)| {
                        let content = if let Some(last_y_range) = &last_y_range
                        {
                            // If the widget's y ranges do not overlap, then add
                            // a new line before appending the selected content.
                            let new_line = if y_range.start()
                                < last_y_range.end()
                                && last_y_range.start() < y_range.end()
                            {
                                ""
                            } else {
                                "\n"
                            };

                            format!("{acc}{new_line}{content}")
                        } else {
                            content
                        };

                        last_y_range = Some(y_range);

                        content
                    },
                );

                if !contents.is_empty() {
                    return (
                        match clipboard_kind {
                            clipboard::ClipboardKind::Standard => {
                                clipboard::write(contents)
                            }
                            clipboard::ClipboardKind::Primary => {
                                clipboard::write_primary(contents)
                            }
                        }
                        .discard(),
                        None,
                    );
                }
            }
            Message::DashboardSaved(Ok(())) => {
                log::debug!("dashboard saved");
            }
            Message::DashboardSaved(Err(error)) => {
                log::warn!("error saving dashboard: {error}");
            }
            Message::Exited(result) => {
                if let Err(error) = result {
                    log::warn!("error saving dashboard on exit: {error}");
                }

                return (Task::none(), Some(Event::Exit));
            }
            Message::Task(message) => {
                let Some(command_bar) = &mut self.command_bar else {
                    return (Task::none(), None);
                };

                match command_bar.update(message) {
                    Some(command_bar::Event::ThemePreview(preview)) => {
                        match preview {
                            Some(preview) => *theme = theme.preview(preview),
                            None => *theme = theme.selected(),
                        }
                    }
                    Some(command_bar::Event::Command(command)) => {
                        let (command, event) = self.handle_command_bar_command(
                            command,
                            backend,
                            theme,
                            config,
                            main_window,
                        );

                        return (
                            Task::batch(vec![
                                command,
                                self.toggle_command_bar(
                                    session, version, config, theme,
                                ),
                            ]),
                            event,
                        );
                    }
                    Some(command_bar::Event::Unfocused) => {
                        return (
                            self.toggle_command_bar(
                                session, version, config, theme,
                            ),
                            None,
                        );
                    }
                    None => {}
                }
            }
            Message::Shortcut(shortcut) => {
                use shortcut::Command::*;

                // Only works on main window / pane_grid
                let mut move_focus = |direction: pane_grid::Direction| {
                    let Focus { window, pane } = self.focus;

                    if window == self.main_window()
                        && let Some(adjacent) =
                            self.panes.main.adjacent(pane, direction)
                    {
                        return self.focus_pane(window, adjacent);
                    }

                    Task::none()
                };

                match shortcut {
                    MoveUp => {
                        return (move_focus(pane_grid::Direction::Up), None);
                    }
                    MoveDown => {
                        return (move_focus(pane_grid::Direction::Down), None);
                    }
                    MoveLeft => {
                        return (move_focus(pane_grid::Direction::Left), None);
                    }
                    MoveRight => {
                        return (move_focus(pane_grid::Direction::Right), None);
                    }
                    NewHorizontalBuffer => {
                        return (
                            self.new_pane(pane_grid::Axis::Horizontal),
                            None,
                        );
                    }
                    NewVerticalBuffer => {
                        return (
                            self.new_pane(pane_grid::Axis::Vertical),
                            None,
                        );
                    }
                    CloseBuffer => {
                        let Focus { window, pane } = self.focus;
                        return (self.close_pane(config, window, pane), None);
                    }
                    MaximizeBuffer => {
                        let Focus { window, pane } = self.focus;
                        // Only main window has >1 pane to maximize
                        if window == self.main_window() {
                            self.panes.main.maximize(pane);
                        }
                    }
                    RestoreBuffer => {
                        self.panes.main.restore();
                    }
                    CycleNextBuffer => {
                        let cycle_buffers = self.cycle_buffers(session, config);

                        let open_buffers = open_buffers(self);

                        if let Some((_, _, state)) = self.get_focused_mut()
                            && let Some(buffer) = cycle_next_buffer(
                                state.buffer.data().as_ref(),
                                cycle_buffers,
                                &open_buffers,
                            )
                        {
                            return (
                                self.open_buffer(
                                    buffer,
                                    BufferAction::ReplacePane,
                                    backend,
                                    config,
                                ),
                                None,
                            );
                        }
                    }
                    CyclePreviousBuffer => {
                        let cycle_buffers = self.cycle_buffers(session, config);

                        let open_buffers = open_buffers(self);

                        if let Some((_, _, state)) = self.get_focused_mut()
                            && let Some(buffer) = cycle_previous_buffer(
                                state.buffer.data().as_ref(),
                                cycle_buffers,
                                &open_buffers,
                            )
                        {
                            return (
                                self.open_buffer(
                                    buffer,
                                    BufferAction::ReplacePane,
                                    backend,
                                    config,
                                ),
                                None,
                            );
                        }
                    }
                    ToggleMemberList => {
                        if let Some((_, _, pane)) = self.get_focused_mut() {
                            if let Some(buffer) = pane.buffer.data() {
                                let settings = self.buffer_settings.entry(
                                    &buffer,
                                    Some(config.buffer.clone().into()),
                                );
                                settings
                                    .conversation
                                    .member_list
                                    .toggle_visibility();
                            }

                            self.last_changed = Some(Instant::now());
                            return (Task::none(), None);
                        }
                    }
                    ToggleDetails => {
                        self.toggle_details_on_focused();
                    }
                    ToggleSidebar => {
                        self.toggle_sidebar();
                    }
                    CommandBar => {
                        return (
                            self.toggle_command_bar(
                                session, version, config, theme,
                            ),
                            None,
                        );
                    }
                    ReloadConfiguration => {
                        return (
                            Task::perform(
                                Config::load(),
                                Message::ConfigReloaded,
                            ),
                            None,
                        );
                    }
                    Logs => {
                        return (
                            self.toggle_internal_buffer(
                                config,
                                data::buffer::Internal::Logs,
                            ),
                            None,
                        );
                    }
                    ThemeEditor => {
                        return (
                            self.toggle_theme_editor(
                                theme,
                                main_window,
                                config,
                            ),
                            None,
                        );
                    }
                    ToggleFullscreen => {
                        return (
                            window::toggle_fullscreen(),
                            Some(Event::ToggleFullscreen),
                        );
                    }
                    QuitApplication => {
                        return (self.exit(config), None);
                    }
                    ScrollUpPage => {
                        return (
                            self.get_focused_mut().map_or_else(
                                Task::none,
                                |(window, pane, state)| {
                                    state.buffer.scroll_up_page().map(
                                        move |message| {
                                            Message::Pane(
                                                window,
                                                pane::Message::Buffer(
                                                    pane, message,
                                                ),
                                            )
                                        },
                                    )
                                },
                            ),
                            None,
                        );
                    }
                    ScrollDownPage => {
                        return (
                            self.get_focused_mut().map_or_else(
                                Task::none,
                                |(window, pane, state)| {
                                    state.buffer.scroll_down_page().map(
                                        move |message| {
                                            Message::Pane(
                                                window,
                                                pane::Message::Buffer(
                                                    pane, message,
                                                ),
                                            )
                                        },
                                    )
                                },
                            ),
                            None,
                        );
                    }
                    ScrollToTop => {
                        return (
                            self.get_focused_mut().map_or_else(
                                Task::none,
                                |(window, id, pane)| {
                                    pane.buffer.scroll_to_start(config).map(
                                        move |message| {
                                            Message::Pane(
                                                window,
                                                pane::Message::Buffer(
                                                    id, message,
                                                ),
                                            )
                                        },
                                    )
                                },
                            ),
                            None,
                        );
                    }
                    ScrollToBottom => {
                        let task = self.get_focused_mut().map_or_else(
                            Task::none,
                            |(window, pane, state)| {
                                let mut task = state
                                    .buffer
                                    .scroll_to_end(config)
                                    .map(move |message| {
                                        Message::Pane(
                                            window,
                                            pane::Message::Buffer(
                                                pane, message,
                                            ),
                                        )
                                    });

                                if config
                                    .buffer
                                    .mark_as_read
                                    .on_scroll_to_bottom
                                {
                                    task =
                                        task.chain(Task::done(Message::Pane(
                                            window,
                                            pane::Message::MarkAsRead,
                                        )));
                                }

                                task
                            },
                        );

                        return (task, None);
                    }
                    CycleNextUnreadBuffer => {
                        let cycle_buffers =
                            self.cycle_buffers_with_has_unread(session, config);

                        let open_buffers = open_buffers(self);

                        if let Some((_, _, state)) = self.get_focused_mut()
                            && let Some(buffer) = cycle_next_unread_buffer(
                                state.buffer.data().as_ref(),
                                cycle_buffers,
                                &open_buffers,
                            )
                        {
                            return (
                                self.open_buffer(
                                    buffer.clone(),
                                    BufferAction::ReplacePane,
                                    backend,
                                    config,
                                ),
                                None,
                            );
                        }
                    }
                    CyclePreviousUnreadBuffer => {
                        let cycle_buffers =
                            self.cycle_buffers_with_has_unread(session, config);

                        let open_buffers = open_buffers(self);

                        if let Some((_, _, state)) = self.get_focused_mut()
                            && let Some(buffer) = cycle_previous_unread_buffer(
                                state.buffer.data().as_ref(),
                                cycle_buffers,
                                &open_buffers,
                            )
                        {
                            return (
                                self.open_buffer(
                                    buffer.clone(),
                                    BufferAction::ReplacePane,
                                    backend,
                                    config,
                                ),
                                None,
                            );
                        }
                    }
                    MarkAsRead => {
                        if let Some((_, _, pane)) = self.get_focused_mut()
                            && let Some(kind) = pane
                                .buffer
                                .data()
                                .and_then(history::Kind::from_buffer)
                        {
                            self.history.mark_as_read(&kind);
                        }
                    }
                    ConfigEditorSave => {
                        let Focus { window, pane } = self.focus;

                        if self.panes.get(window, pane).is_some_and(|state| {
                            matches!(
                                &state.buffer,
                                Buffer::ConfigEditor(editor)
                                    if editor.has_unsaved_changes()
                            )
                        }) {
                            return (
                                Task::done(Message::Pane(
                                    window,
                                    pane::Message::Buffer(
                                        pane,
                                        buffer::Message::ConfigEditor(
                                            buffer::config_editor::Message::Save,
                                        ),
                                    ),
                                )),
                                None,
                            );
                        }
                    }
                    OpenConfigEditor => {
                        return (
                            self.toggle_internal_buffer(
                                config,
                                data::buffer::Internal::ConfigEditor,
                            ),
                            None,
                        );
                    }
                    OpenConfigFile => {
                        let _ = open_url::open(Config::path());
                    }
                    // Sidebar-only toggles; nothing to do from a pane.
                    ShowMutedBuffers | HideMutedBuffers => {}
                }
            }
            Message::CloseContextMenu(window, any_closed) => {
                if !any_closed {
                    if let Some((_, _, state)) = self.get_focused_mut()
                        && state.buffer.close_picker()
                    {
                        return (Task::none(), None);
                    }

                    if self.is_pane_maximized() && window == self.main_window()
                    {
                        self.panes.main.restore();
                    }
                }
            }
            Message::ThemeEditor(message) => {
                let mut editor_event = None;
                let mut event = None;
                let mut tasks = vec![];

                if let Some(editor) = self.theme_editor.as_mut() {
                    let (task, event) = editor.update(message, theme);

                    tasks.push(task.map(Message::ThemeEditor));
                    editor_event = event;
                }

                if let Some(editor_event) = editor_event {
                    match editor_event {
                        theme_editor::Event::Close => {
                            tasks.push(self.close_theme_editor(theme));
                        }
                        theme_editor::Event::ReloadThemes => {
                            event = Some(Event::ReloadThemes);
                        }
                    }
                }

                return (Task::batch(tasks), event);
            }
            Message::ConfigReloaded(config_result) => {
                for (_, _, state) in self.panes.iter_mut() {
                    if let Buffer::ConfigEditor(editor) = &mut state.buffer {
                        editor.config_reloaded(config_result.as_ref().err());
                    }
                }

                return (
                    Task::none(),
                    Some(Event::ConfigReloaded(config_result)),
                );
            }
            Message::ConfigEditorReloaded(config_result) => {
                for (_, _, state) in self.panes.iter_mut() {
                    if let Buffer::ConfigEditor(editor) = &mut state.buffer {
                        editor.config_reloaded(config_result.as_ref().err());
                    }
                }

                // Errors are surfaced within the config editor instead of
                // the reload error modal.
                let event = config_result
                    .is_ok()
                    .then_some(Event::ConfigReloaded(config_result));

                return (Task::none(), event);
            }
            Message::NewWindow(window, pane) => {
                let (state, pane) = pane_grid::State::new(pane);
                self.panes.popout.insert(window, state);

                return (self.focus_pane(window, pane), None);
            }
            Message::StatusBar(message) => {
                self.status_bar.update(message);
            }
        }

        (Task::none(), None)
    }

    pub fn view_window<'a>(
        &'a self,
        window: window::Id,
        session: &'a data::Session,
        version: &'a Version,
        config: &'a Config,
        theme: &'a Theme,
    ) -> Element<'a, Message> {
        if let Some(state) = self.panes.popout.get(&window) {
            let pane_gap = config.pane.gap.outer;
            let top_padding =
                platform_specific::popped_out_window_padding(config) + pane_gap;
            let padding = padding::all(pane_gap).top(top_padding as f32);

            let content = container(
                PaneGrid::new(state, |id, pane, _maximized| {
                    let is_focused = self.focus == Focus { window, pane: id };
                    let buffer = pane.buffer.data();
                    let settings = buffer
                        .as_ref()
                        .and_then(|b| self.buffer_settings.get(b));

                    pane.view(
                        id,
                        1,
                        is_focused,
                        false,
                        session,
                        &self.history,
                        &self.side_menu,
                        config,
                        theme,
                        settings,
                        window != self.main_window(),
                    )
                })
                .spacing(config.pane.gap.inner)
                .on_click(pane::Message::PaneClicked),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(padding);

            let base = Element::new(content)
                .map(move |message| Message::Pane(window, message));
            let base = self.with_command_bar_overlay(
                base, window, session, version, config,
            );

            return self.with_keyboard_shortcuts(base, config);
        } else if let Some(editor) = self.theme_editor.as_ref()
            && editor.window == window
        {
            return editor.view(config, theme).map(Message::ThemeEditor);
        }

        column![].into()
    }

    pub fn view<'a>(
        &'a self,
        session: &'a data::Session,
        version: &'a Version,
        config: &'a Config,
        theme: &'a Theme,
    ) -> Element<'a, Message> {
        let pane_grid: Element<_> =
            PaneGrid::new(&self.panes.main, |id, pane, maximized| {
                let is_focused = self.focus
                    == Focus {
                        window: self.main_window(),
                        pane: id,
                    };
                let panes = self.panes.main.panes.len();
                let buffer = pane.buffer.data();
                let settings =
                    buffer.as_ref().and_then(|b| self.buffer_settings.get(b));

                pane.view(
                    id,
                    panes,
                    is_focused,
                    maximized,
                    session,
                    &self.history,
                    &self.side_menu,
                    config,
                    theme,
                    settings,
                    false,
                )
            })
            .on_click(pane::Message::PaneClicked)
            .on_resize(6, pane::Message::PaneResized)
            .on_drag(pane::Message::PaneDragged)
            .spacing(config.pane.gap.inner)
            .into();

        let pane_padding = match config.sidebar.position {
            data::config::sidebar::Position::Left => {
                padding::all(config.pane.gap.outer).left(config.pane.gap.inner)
            }
            data::config::sidebar::Position::Top => {
                padding::all(config.pane.gap.outer).top(config.pane.gap.inner)
            }
            data::config::sidebar::Position::Right => {
                padding::all(config.pane.gap.outer).right(config.pane.gap.inner)
            }
            data::config::sidebar::Position::Bottom => {
                padding::all(config.pane.gap.outer)
                    .bottom(config.pane.gap.inner)
            }
        };

        let pane_grid =
            container(pane_grid.map(move |message| {
                Message::Pane(self.main_window(), message)
            }))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(pane_padding);

        let side_menu = self
            .side_menu
            .view(
                session,
                &self.history,
                &self.panes,
                self.focus,
                config,
                version,
                theme,
            )
            .map(|e| e.map(Message::Sidebar));

        let content = match config.sidebar.position {
            data::config::sidebar::Position::Left
            | data::config::sidebar::Position::Top => {
                vec![
                    side_menu.unwrap_or_else(|| row![].into()),
                    pane_grid.into(),
                ]
            }
            data::config::sidebar::Position::Right
            | data::config::sidebar::Position::Bottom => {
                vec![
                    pane_grid.into(),
                    side_menu.unwrap_or_else(|| row![].into()),
                ]
            }
        };

        let base: Element<Message> = if config.sidebar.position.is_horizontal()
        {
            Column::with_children(content)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            Row::with_children(content)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };

        // The status strip claims height only while it has something to
        // say (backend errors, delivery not online).
        let base: Element<Message> =
            match status_bar::view(&self.status_bar, session, theme) {
                Some(strip) => column![
                    container(base).width(Length::Fill).height(Length::Fill),
                    strip.map(Message::StatusBar),
                ]
                .into(),
                None => base,
            };

        let base = self.with_command_bar_overlay(
            base,
            self.main_window(),
            session,
            version,
            config,
        );

        self.with_keyboard_shortcuts(base, config)
    }

    fn with_command_bar_overlay<'a>(
        &'a self,
        base: Element<'a, Message>,
        window: window::Id,
        session: &'a data::Session,
        version: &'a Version,
        config: &'a Config,
    ) -> Element<'a, Message> {
        let base = if self.command_bar_window == Some(window)
            && let Some(command_bar) = self.command_bar.as_ref()
        {
            let background = anchored_overlay(
                base,
                container(
                    Space::new().width(Length::Fill).height(Length::Fill),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::container::transparent_overlay),
                anchored_overlay::Anchor::BelowTopCentered,
                0.0,
            );

            anchored_overlay(
                background,
                command_bar
                    .view(
                        session,
                        self.focus,
                        self.buffer_resize_action(),
                        version,
                        config,
                        self.main_window(),
                    )
                    .map(Message::Task),
                anchored_overlay::Anchor::BelowTopCentered,
                10.0,
            )
        } else {
            // Align `base` into same view tree shape
            // as `anchored_overlay` to prevent diff
            // from firing when displaying command bar
            column![column![base]].into()
        };

        // Wrap in stack so iced can track base properly
        stack![base].into()
    }

    fn with_keyboard_shortcuts<'a>(
        &'a self,
        base: Element<'a, Message>,
        config: &'a Config,
    ) -> Element<'a, Message> {
        shortcut(base, config.keyboard.shortcuts(), Message::Shortcut)
    }

    pub fn handle_buffer_event(
        &mut self,
        window: window::Id,
        id: pane_grid::Pane,
        event: buffer::Event,
        session: &data::Session,
        backend: &mut stream::Map,
        config: &Config,
    ) -> (Task<Message>, Option<Event>) {
        let close_context_menu = move || {
            context_menu::close(convert::identity).map(move |any_closed| {
                Message::CloseContextMenu(window, any_closed)
            })
        };

        match event {
            buffer::Event::ContextMenu(event) => {
                let action = match event {
                    buffer::context_menu::Event::CopyText(text) => {
                        clipboard::write(text).discard()
                    }
                    buffer::context_menu::Event::DmWith(address) => {
                        self.open_dm(&address, session, backend, config)
                    }
                };

                return (Task::batch(vec![close_context_menu(), action]), None);
            }
            buffer::Event::CopyText(text) => {
                return (
                    Task::batch(vec![
                        close_context_menu(),
                        clipboard::write(text).discard(),
                    ]),
                    None,
                );
            }
            buffer::Event::OpenDm(address) => {
                return (
                    Task::batch(vec![
                        close_context_menu(),
                        self.open_dm(&address, session, backend, config),
                    ]),
                    None,
                );
            }
            buffer::Event::OpenAddMember(convo_id) => {
                return (
                    close_context_menu(),
                    Some(Event::OpenAddMember(convo_id)),
                );
            }
            buffer::Event::OpenUrl(url) => {
                return (
                    Task::none(),
                    Some(Event::OpenUrl(
                        url,
                        config.buffer.url.prompt_before_open,
                    )),
                );
            }
            buffer::Event::MarkAsRead(kind) => {
                self.history.mark_as_read(&kind);
            }
            buffer::Event::SendMessage { convo_id, content } => {
                backend.send(stream::Control::SendMessage {
                    convo_id: wire_id(&convo_id),
                    content,
                });
            }
            buffer::Event::Command(command) => {
                return self
                    .handle_command(command, window, id, session, backend);
            }
            buffer::Event::ConfigSaved => {
                return (
                    Task::perform(
                        Config::load(),
                        Message::ConfigEditorReloaded,
                    ),
                    None,
                );
            }
        }

        (Task::none(), None)
    }

    /// Composer slash commands. dm/group/add/nick map onto backend
    /// controls; the results come back as push events (e.g.
    /// `conversation_created`). Missing arguments open the matching
    /// dialog instead of erroring.
    fn handle_command(
        &mut self,
        command: data::Command,
        window: window::Id,
        id: pane_grid::Pane,
        session: &data::Session,
        backend: &mut stream::Map,
    ) -> (Task<Message>, Option<Event>) {
        let convo_id = self
            .panes
            .get(window, id)
            .and_then(|pane| pane.buffer.convo_id().cloned());

        match command {
            data::Command::Dm(Some(peer_address)) => {
                backend
                    .send(stream::Control::CreateConversation { peer_address });
            }
            data::Command::Dm(None) => {
                return (Task::none(), Some(Event::OpenNewDm));
            }
            data::Command::Group(Some(name), description) => {
                backend.send(stream::Control::CreateGroup {
                    name,
                    description: description.unwrap_or_default(),
                });
            }
            data::Command::Group(None, _) => {
                return (Task::none(), Some(Event::OpenNewGroup));
            }
            data::Command::Add(peer_address) => {
                // Membership is a group concept; QML only reaches add-member
                // from the group roster pane.
                let group_id = convo_id.filter(|convo_id| {
                    session.conversations.get(convo_id).is_some_and(
                        |conversation| {
                            conversation.kind == ConversationKind::Group
                        },
                    )
                });

                match group_id {
                    Some(convo_id) => {
                        return (
                            Task::none(),
                            Some(Event::AddMemberRequested {
                                convo_id,
                                peer_address,
                            }),
                        );
                    }
                    None => self.report_error(
                        "/add only works in group conversations".to_string(),
                    ),
                }
            }
            data::Command::Nick(Some(nickname)) => {
                if let Some(convo_id) = convo_id {
                    backend.send(stream::Control::SetNickname {
                        convo_id: wire_id(&convo_id),
                        nickname,
                    });
                }
            }
            data::Command::Nick(None) => {
                if let Some(convo_id) = convo_id {
                    return (
                        Task::none(),
                        Some(Event::OpenSetNickname(convo_id)),
                    );
                }
            }
            data::Command::Details => {
                if let Some(state) = self.panes.get_mut(window, id)
                    && let Buffer::Conversation(conversation) =
                        &mut state.buffer
                {
                    conversation.toggle_details();
                }
            }
            data::Command::Clear => {
                if let Some(convo_id) = convo_id {
                    self.history
                        .clear_messages(&history::Kind::Conversation(convo_id));
                }
            }
        }

        (Task::none(), None)
    }

    pub fn handle_event(
        &mut self,
        window: window::Id,
        event: event::Event,
        session: &data::Session,
        version: &Version,
        config: &Config,
        theme: &mut Theme,
    ) -> Task<Message> {
        use event::Event::*;

        match event {
            Escape => {
                // Order of operations
                //
                // - Close command bar (if this window owns it)
                // - Close theme editor (if this window owns it)
                // - Close context menu
                // - Restore maximized pane (if main window)
                if self.command_bar_window == Some(window) {
                    self.toggle_command_bar(session, version, config, theme)
                } else if self.theme_editor.as_ref().map(|e| e.window)
                    == Some(window)
                {
                    self.close_theme_editor(theme)
                } else {
                    context_menu::close(convert::identity).map(
                        move |any_closed| {
                            Message::CloseContextMenu(window, any_closed)
                        },
                    )
                }
            }
            Copy => selectable_text::selected(|selected_text| {
                Message::SelectedText(
                    selected_text,
                    clipboard::ClipboardKind::Standard,
                )
            }),
            LeftClick => self.refocus_pane(),
            UpdatePrimaryClipboard => {
                selectable_text::selected(|selected_text| {
                    Message::SelectedText(
                        selected_text,
                        clipboard::ClipboardKind::Primary,
                    )
                })
            }
        }
    }

    fn handle_command_bar_command(
        &mut self,
        command: command_bar::Command,
        backend: &mut stream::Map,
        theme: &mut Theme,
        config: &Config,
        main_window: &Window,
    ) -> (Task<Message>, Option<Event>) {
        match command {
            command_bar::Command::Version(command) => match command {
                command_bar::Version::Application(_) => {
                    let _ = open_url::open(RELEASE_WEBSITE);
                    (Task::none(), None)
                }
            },
            command_bar::Command::Buffer(command) => match command {
                command_bar::Buffer::Maximize(_) => {
                    self.maximize_pane();
                    (Task::none(), None)
                }
                command_bar::Buffer::NewHorizontal => {
                    (self.new_pane(pane_grid::Axis::Horizontal), None)
                }
                command_bar::Buffer::NewVertical => {
                    (self.new_pane(pane_grid::Axis::Vertical), None)
                }
                command_bar::Buffer::Close => {
                    let Focus { window, pane } = self.focus;
                    (self.close_pane(config, window, pane), None)
                }
                command_bar::Buffer::Replace(buffer, _) => (
                    self.open_buffer(
                        buffer,
                        BufferAction::ReplacePane,
                        backend,
                        config,
                    ),
                    None,
                ),
                command_bar::Buffer::Popout => (self.popout_pane(config), None),
                command_bar::Buffer::Merge => (self.merge_pane(config), None),
            },
            command_bar::Command::Configuration(command) => match command {
                command_bar::Configuration::OpenConfigDirectory => {
                    let _ = open_url::open(Config::config_dir());
                    (Task::none(), None)
                }
                command_bar::Configuration::OpenCacheDirectory => {
                    let _ = open_url::open(environment::cache_dir());
                    (Task::none(), None)
                }
                command_bar::Configuration::OpenDataDirectory => {
                    let _ = open_url::open(environment::data_dir());
                    (Task::none(), None)
                }
                command_bar::Configuration::OpenWebsite => {
                    let _ = open_url::open(environment::WIKI_WEBSITE);
                    (Task::none(), None)
                }
                command_bar::Configuration::Reload => (
                    Task::perform(Config::load(), Message::ConfigReloaded),
                    None,
                ),
                command_bar::Configuration::OpenConfigFile => {
                    let _ = open_url::open(Config::path());
                    (Task::none(), None)
                }
            },
            command_bar::Command::Theme(command) => match command {
                command_bar::Theme::Switch(new) => {
                    *theme = Theme::from(new);
                    (Task::none(), None)
                }
                command_bar::Theme::OpenEditor => {
                    if let Some(editor) = &self.theme_editor {
                        (window::gain_focus(editor.window), None)
                    } else {
                        let (editor, task) =
                            ThemeEditor::open(main_window, config);

                        self.theme_editor = Some(editor);

                        (task.then(|_| Task::none()), None)
                    }
                }
                command_bar::Theme::OpenThemesWebsite => {
                    let _ = open_url::open(environment::THEME_WEBSITE);
                    (Task::none(), None)
                }
            },
            command_bar::Command::Application(application) => match application
            {
                command_bar::Application::Quit => (self.exit(config), None),
                command_bar::Application::ToggleFullscreen => {
                    (window::toggle_fullscreen(), Some(Event::ToggleFullscreen))
                }
                command_bar::Application::ToggleSidebarVisibility => {
                    self.toggle_sidebar();
                    (Task::none(), None)
                }
            },
        }
    }

    fn close_theme_editor(&mut self, theme: &mut Theme) -> Task<Message> {
        if let Some(editor) = self.theme_editor.take() {
            *theme = theme.selected();
            window::close(editor.window)
        } else {
            Task::none()
        }
    }

    fn toggle_theme_editor(
        &mut self,
        theme: &mut Theme,
        main_window: &Window,
        config: &Config,
    ) -> Task<Message> {
        if self.theme_editor.is_some() {
            self.close_theme_editor(theme)
        } else {
            let (editor, task) = ThemeEditor::open(main_window, config);

            self.theme_editor = Some(editor);

            task.then(|_| Task::none())
        }
    }

    fn toggle_internal_buffer(
        &mut self,
        config: &Config,
        buffer: data::buffer::Internal,
    ) -> Task<Message> {
        let open = self.panes.iter().find_map(|(window_id, pane, state)| {
            (state.buffer.internal().as_ref() == Some(&buffer))
                .then_some((window_id, pane))
        });

        if let Some((window, pane)) = open {
            self.close_pane(config, window, pane)
        } else {
            self.open_buffer(
                buffer.into(),
                config.actions.buffer.open_internal,
                &mut stream::Map::default(),
                config,
            )
        }
    }

    /// Opens a conversation buffer; the standard entry point for sidebar
    /// clicks, notifications, and backend-created conversations.
    pub fn open_conversation(
        &mut self,
        convo_id: ConvoId,
        buffer_action: BufferAction,
        backend: &mut stream::Map,
        config: &Config,
    ) -> Task<Message> {
        self.open_buffer(
            data::Buffer::Conversation(convo_id),
            buffer_action,
            backend,
            config,
        )
    }

    fn open_buffer(
        &mut self,
        buffer: data::Buffer,
        buffer_action: BufferAction,
        backend: &mut stream::Map,
        config: &Config,
    ) -> Task<Message> {
        // TODO(post-v1): reduce clones — the pane grid is cloned per open to
        // keep the borrow checker happy while `self` is mutated below.
        let panes = self.panes.clone();

        self.last_changed = Some(Instant::now());

        let task = match buffer_action {
            BufferAction::ReplacePane => {
                // If buffer already is open, we swap it with focused pane.
                for (window, id, pane) in panes.iter() {
                    if pane.buffer.data().as_ref() == Some(&buffer) {
                        if window != self.focus.window || id != self.focus.pane
                        {
                            return self.swap_pane_with_focus(window, id);
                        } else {
                            return Task::none();
                        }
                    }
                }

                let Focus { window, pane } = self.focus;

                if let Some(state) = self.panes.get_mut(window, pane) {
                    mark_as_read_on_buffer_close(
                        &state.buffer,
                        &mut self.history,
                        config,
                    );

                    state.buffer = Buffer::from_data(
                        buffer,
                        &self.history,
                        state.size,
                        config,
                    );
                    self.last_changed = Some(Instant::now());

                    Task::batch(vec![
                        self.reset_pane(window, pane),
                        self.focus_pane(window, pane),
                    ])
                } else {
                    log::error!("Didn't find any panes to replace");
                    Task::none()
                }
            }
            BufferAction::NewPane => {
                // If buffer already is open, we focus it.
                let mut existing = None;
                for (window, id, pane) in panes.iter() {
                    if pane.buffer.data().as_ref() == Some(&buffer) {
                        existing = Some((window, id));
                        break;
                    }
                }

                if let Some((window, id)) = existing {
                    self.focus = Focus { window, pane: id };

                    self.focus_pane(window, id)
                } else {
                    self.new_pane_with_buffer(buffer, config)
                }
            }
            BufferAction::NewWindow => {
                iced::window::position(self.main_window()).then({
                    let pane = Pane::new(Buffer::from_data(
                        buffer.clone(),
                        &self.history,
                        Size::default(),
                        config,
                    ));

                    let config = config.clone();
                    move |main_window_position| {
                        let (_, task) = window::open(window::Settings {
                            // Just big enough to show all components in combobox
                            position: main_window_position
                                .map(|point| {
                                    window::Position::Specific(
                                        point + Vector::new(20.0, 20.0),
                                    )
                                })
                                .unwrap_or_default(),
                            exit_on_close_request: false,
                            ..window::settings(&config)
                        });

                        task.map({
                            let pane = pane.clone();
                            move |id| Message::NewWindow(id, pane.clone())
                        })
                    }
                })
            }
        };

        self.track_histories(backend);

        task
    }

    fn new_pane_with_buffer(
        &mut self,
        buffer: data::Buffer,
        config: &Config,
    ) -> Task<Message> {
        // If we only have one pane, and its empty, we replace it.
        if self.panes.len() == 1 {
            let empty = self.panes.main.iter().find_map(|(id, pane)| {
                matches!(pane.buffer, Buffer::Empty).then_some(*id)
            });

            if let Some(id) = empty {
                let size = self
                    .panes
                    .main
                    .get(id)
                    .map(|pane| pane.size)
                    .unwrap_or_default();

                if let Some(pane) = self.panes.main.get_mut(id) {
                    pane.buffer =
                        Buffer::from_data(buffer, &self.history, size, config);
                }
                self.last_changed = Some(Instant::now());

                return self.focus_pane(self.main_window(), id);
            }
        }

        let (pane_to_split, pane_to_split_state) = {
            if matches!(
                config.pane.split_axis,
                config::pane::SplitAxis::LargestShorter
            ) && let Some((pane, pane_state)) =
                self.panes.main.panes.iter().reduce(
                    |(acc_pane, acc_pane_state), (pane, pane_state)| {
                        let pane_area =
                            pane_state.size.width * pane_state.size.height;
                        let acc_pane_area = acc_pane_state.size.width
                            * acc_pane_state.size.height;

                        if pane_area > acc_pane_area {
                            (pane, pane_state)
                        } else {
                            (acc_pane, acc_pane_state)
                        }
                    },
                )
            {
                (*pane, pane_state)
            } else if self.focus.window == self.main_window()
                && let Some(pane_state) =
                    self.panes.main.panes.get(&self.focus.pane)
            {
                (self.focus.pane, pane_state)
            } else if let Some((pane, pane_state)) =
                self.panes.main.panes.iter().last()
            {
                (*pane, pane_state)
            } else {
                log::error!("Didn't find any panes to split");
                return Task::none();
            }
        };

        let split_axis = match config.pane.split_axis {
            config::pane::SplitAxis::Horizontal => pane_grid::Axis::Horizontal,
            config::pane::SplitAxis::Vertical => pane_grid::Axis::Vertical,
            config::pane::SplitAxis::Shorter
            | config::pane::SplitAxis::LargestShorter => {
                if pane_to_split_state.size.height
                    < pane_to_split_state.size.width
                {
                    pane_grid::Axis::Vertical
                } else {
                    pane_grid::Axis::Horizontal
                }
            }
        };

        let result = self.panes.main.split(
            split_axis,
            pane_to_split,
            Pane::new(Buffer::from_data(
                buffer,
                &self.history,
                pane_to_split_state.size,
                config,
            )),
        );

        if let Some((pane, _)) = result {
            return self.focus_pane(self.main_window(), pane);
        }

        Task::none()
    }

    /// A conversation vanished (deleted locally or remotely): panes showing
    /// it become Empty and its history is dropped.
    pub fn conversation_deleted(&mut self, convo_id: &ConvoId) {
        self.history
            .close(&history::Kind::Conversation(convo_id.clone()));

        for (_, _, pane) in self.panes.iter_mut() {
            if pane.buffer.convo_id() == Some(convo_id) {
                pane.buffer = Buffer::Empty;
            }
        }

        self.last_changed = Some(Instant::now());
    }

    /// Restores a failed send's text into the conversation's composer.
    /// The stored draft is written too (it was cleared at send time), so
    /// the text survives a pane switch — and lands at all when no pane
    /// currently shows the conversation.
    pub fn restore_draft(&mut self, convo_id: &ConvoId, content: &str) {
        if self.history.input(convo_id).draft_message.trim().is_empty() {
            self.history.record_draft(RawInput {
                convo_id: convo_id.clone(),
                text: content.to_string(),
            });
        }

        for (_, _, pane) in self.panes.iter_mut() {
            pane.buffer.restore_draft(convo_id, content);
        }
    }

    /// Queues a user-visible error onto the status bar.
    pub fn report_error(&mut self, error: String) {
        self.status_bar.push_error(error);
    }

    pub fn member_add_explained(&self) -> bool {
        self.member_add_explained
    }

    /// Persists the first-add explainer acknowledgement (saved with the
    /// dashboard).
    pub fn set_member_add_explained(&mut self) {
        self.member_add_explained = true;
        self.last_changed = Some(Instant::now());
    }

    /// Toggles the focused conversation pane's details panel.
    fn toggle_details_on_focused(&mut self) {
        if let Some((_, _, pane)) = self.get_focused_mut()
            && let Buffer::Conversation(conversation) = &mut pane.buffer
        {
            conversation.toggle_details();
        }
    }

    /// Opens the direct conversation with `address` when one exists;
    /// otherwise asks the backend to create it (the `conversation_created`
    /// push event then focuses the new pane).
    fn open_dm(
        &mut self,
        address: &str,
        session: &data::Session,
        backend: &mut stream::Map,
        config: &Config,
    ) -> Task<Message> {
        let existing = session.conversations.iter().find(|conversation| {
            conversation.kind == ConversationKind::Direct
                && conversation
                    .peer_address()
                    .is_some_and(|peer| peer.as_str() == address)
        });

        if let Some(conversation) = existing {
            self.open_conversation(
                conversation.id.clone(),
                BufferAction::ReplacePane,
                backend,
                config,
            )
        } else {
            backend.send(stream::Control::CreateConversation {
                peer_address: address.to_string(),
            });

            Task::none()
        }
    }

    pub fn record_message(
        &mut self,
        kind: history::Kind,
        message: data::Message,
    ) {
        self.history.record_message(kind, message);
    }

    pub fn load_messages(
        &mut self,
        kind: history::Kind,
        messages: Vec<data::Message>,
    ) -> Task<Message> {
        self.history.load_full(kind.clone(), messages);

        // A thread's settle window runs from the moment its rows land, so
        // the panes showing it have to hear about it.
        Task::batch(self.panes.iter_mut().map(|(window, pane, state)| {
            state.buffer.messages_loaded(&kind).map(move |message| {
                Message::Pane(window, pane::Message::Buffer(pane, message))
            })
        }))
    }

    pub fn mark_unread(&mut self, kind: &history::Kind) {
        self.history.mark_unread(kind);
    }

    pub fn record_log(&mut self, record: data::log::Record) {
        self.history.record_log(record);
    }

    pub fn get_focused(&self) -> Option<(window::Id, pane_grid::Pane, &Pane)> {
        let Focus { window, pane } = self.focus;
        self.panes
            .get(window, pane)
            .map(|state| (window, pane, state))
    }

    fn get_focused_mut(
        &mut self,
    ) -> Option<(window::Id, pane_grid::Pane, &mut Pane)> {
        let Focus { window, pane } = self.focus;
        self.panes
            .get_mut(window, pane)
            .map(|state| (window, pane, state))
    }

    pub fn refocus_pane(&mut self) -> Task<Message> {
        let Focus { window, pane } = self.focus;

        return self
            .panes
            .iter()
            .find_map(|(w, p, state)| {
                (w == window && p == pane).then(|| {
                    state.buffer.focus().map(move |message| {
                        Message::Pane(
                            window,
                            pane::Message::Buffer(pane, message),
                        )
                    })
                })
            })
            .unwrap_or_else(Task::none);
    }

    fn focus_pane(
        &mut self,
        window: window::Id,
        pane: pane_grid::Pane,
    ) -> Task<Message> {
        if (self.focus != Focus { window, pane })
            || self.focus_history.is_empty()
        {
            self.focus = Focus { window, pane };

            self.last_changed = Some(Instant::now());

            if window == self.main_window() {
                self.focus_history.push_front(pane);

                self.focus_history.truncate(FOCUS_HISTORY_LEN);

                if self.is_pane_maximized() {
                    self.panes.main.restore();
                    self.panes.main.maximize(pane);
                }
            }
        }

        self.refocus_pane()
    }

    fn focus_first_pane(&mut self, window: window::Id) -> Task<Message> {
        let pane = self
            .panes
            .iter()
            .find_map(|(w, pane, _)| (w == window).then_some(pane));

        pane.map_or(Task::none(), |pane| self.focus_pane(window, pane))
    }

    pub fn focus_window_pane(&mut self, window: window::Id) -> Task<Message> {
        if self.focus.window == window {
            Task::none()
        } else if let Some(pane) = self.focus_history.front()
            && window == self.main_window()
        {
            self.focus_pane(window, *pane)
        } else {
            self.focus_first_pane(window)
        }
    }

    pub fn focus_window(&mut self, window: window::Id) -> Task<Message> {
        let task = self.focus_window_pane(window);

        window::gain_focus(window).chain(task)
    }

    fn maximize_pane(&mut self) {
        if self.is_pane_maximized() {
            self.panes.main.restore();
        } else if self.focus.window == self.main_window() {
            self.panes.main.maximize(self.focus.pane);
        }
    }

    fn is_pane_maximized(&self) -> bool {
        self.panes.main.maximized().is_some()
    }

    fn new_pane(&mut self, axis: pane_grid::Axis) -> Task<Message> {
        if self.focus.window == self.main_window() {
            // If there is any focused pane on main window, split it
            return self.split_pane(axis);
        } else {
            // If there is no focused pane, split the last pane or create a new empty grid
            let pane =
                self.panes.main.iter().last().map(|(pane, _)| pane).copied();

            if let Some(pane) = pane {
                let result =
                    self.panes.main.split(axis, pane, Pane::new(Buffer::Empty));
                self.last_changed = Some(Instant::now());

                if let Some((pane, _)) = result {
                    return self.focus_pane(self.main_window(), pane);
                }
            } else {
                let (state, pane) =
                    pane_grid::State::new(Pane::new(Buffer::Empty));
                self.panes.main = state;
                self.last_changed = Some(Instant::now());
                return self.focus_pane(self.main_window(), pane);
            }
        }

        Task::none()
    }

    fn split_pane(&mut self, axis: pane_grid::Axis) -> Task<Message> {
        if self.focus.window == self.main_window() {
            let result = self.panes.main.split(
                axis,
                self.focus.pane,
                Pane::new(Buffer::Empty),
            );
            self.last_changed = Some(Instant::now());
            if let Some((pane, _)) = result {
                return self.focus_pane(self.main_window(), pane);
            }
        }

        Task::none()
    }

    fn reset_pane(
        &mut self,
        window: window::Id,
        pane: pane_grid::Pane,
    ) -> Task<Message> {
        if let Some(state) = self.panes.get_mut(window, pane) {
            state.buffer.reset();
        }

        Task::none()
    }

    fn close_pane(
        &mut self,
        config: &Config,
        window: window::Id,
        pane: pane_grid::Pane,
    ) -> Task<Message> {
        let mut tasks = vec![];

        if let Some(state) = self.panes.get(window, pane) {
            mark_as_read_on_buffer_close(
                &state.buffer,
                &mut self.history,
                config,
            );

            if config.buffer.close.direct.close()
                && let Some(history::Kind::Conversation(convo_id)) =
                    state.buffer.data().and_then(history::Kind::from_buffer)
            {
                self.history.close(&history::Kind::Conversation(convo_id));
            }
        }

        self.last_changed = Some(Instant::now());

        if window == self.main_window() {
            self.focus_history.retain(|p| *p != pane);

            if let Some((_, sibling)) = self.panes.main.close(pane) {
                if (Focus { window, pane } == self.focus) {
                    tasks.push(self.focus_pane(self.main_window(), sibling));
                    return Task::batch(tasks);
                }
            } else if let Some(pane) = self.panes.main.get_mut(pane) {
                pane.buffer = Buffer::Empty;
            }
        } else if self.panes.popout.remove(&window).is_some() {
            if self.command_bar_window == Some(window) {
                self.close_command_bar();
            }

            tasks.push(
                window::close(window)
                    .chain(self.focus_window(self.main_window())),
            );
            return Task::batch(tasks);
        }

        Task::batch(tasks)
    }

    fn popout_pane(&mut self, config: &Config) -> Task<Message> {
        let Focus { pane, .. } = self.focus;

        self.focus_history.retain(|p| *p != pane);

        if let Some((pane, _)) = self.panes.main.close(pane)
            && let Some(buffer) = pane.buffer.data()
        {
            return self.open_buffer(
                buffer,
                BufferAction::NewWindow,
                &mut stream::Map::default(),
                config,
            );
        }

        Task::none()
    }

    fn merge_pane(&mut self, config: &Config) -> Task<Message> {
        let Focus { window, pane } = self.focus;

        if let Some(pane) = self
            .panes
            .popout
            .remove(&window)
            .and_then(|panes| panes.get(pane).cloned())
        {
            let task = match pane.buffer.data() {
                Some(buffer) => self.open_buffer(
                    buffer,
                    BufferAction::NewPane,
                    &mut stream::Map::default(),
                    config,
                ),
                None => self.new_pane(pane_grid::Axis::Horizontal),
            };

            return Task::batch(vec![
                window::close(window),
                window::gain_focus(self.main_window()).chain(task),
            ]);
        }

        Task::none()
    }

    fn swap_pane_with_focus(
        &mut self,
        from_window: window::Id,
        from_pane: pane_grid::Pane,
    ) -> Task<Message> {
        self.last_changed = Some(Instant::now());

        let Focus {
            window: to_window,
            pane: to_pane,
        } = self.focus;

        if from_window == self.main_window() && to_window == self.main_window()
        {
            self.panes.main.swap(from_pane, to_pane);

            self.focus_pane(from_window, from_pane)
        } else {
            if let Some((from_state, to_state)) = self
                .panes
                .get(from_window, from_pane)
                .cloned()
                .zip(self.panes.get(to_window, to_pane).cloned())
            {
                if let Some(state) = self.panes.get_mut(from_window, from_pane)
                {
                    *state = to_state;
                }
                if let Some(state) = self.panes.get_mut(to_window, to_pane) {
                    *state = from_state;
                }
            }

            Task::none()
        }
    }

    /// Syncs tracked histories with the open panes and requests message
    /// loads for conversation panes whose history is not `Full` yet (the
    /// module is the message store; `MessagesLoaded` folds the reply in).
    pub fn track_histories(&mut self, backend: &mut stream::Map) {
        let resources: HashSet<history::Resource> =
            self.panes.resources().collect();

        for resource in &resources {
            if let history::Kind::Conversation(convo_id) = &resource.kind
                && self.history.get_messages(&resource.kind, None).is_none()
            {
                backend.send(stream::Control::LoadMessages {
                    convo_id: wire_id(convo_id),
                });
            }
        }

        self.history.track(resources);
    }

    /// Once-per-open fallback fetch: the snapshot-time member loads are
    /// best-effort (a full control queue drops them), so conversation
    /// panes re-request their roster while it is still unknown. Deduped
    /// per snapshot generation via [`Self::reset_member_requests`].
    pub fn track_member_loads(
        &mut self,
        session: &data::Session,
        backend: &mut stream::Map,
    ) {
        let convo_ids: Vec<ConvoId> = self
            .panes
            .iter()
            .filter_map(|(_, _, pane)| pane.buffer.convo_id().cloned())
            .collect();

        for convo_id in convo_ids {
            if session
                .conversations
                .get(&convo_id)
                .is_some_and(|conversation| conversation.members.is_empty())
                && self.requested_member_loads.insert(convo_id.clone())
            {
                backend.send(stream::Control::LoadMembers {
                    convo_id: wire_id(&convo_id),
                });
            }
        }
    }

    /// Starts a fresh member-request generation; each snapshot replaces
    /// the conversation set, so earlier dedupe entries are stale.
    pub fn reset_member_requests(&mut self) {
        self.requested_member_loads.clear();
    }

    pub fn tick(&mut self, now: Instant) -> Task<Message> {
        if let Some(last_changed) = self.last_changed
            && now.duration_since(last_changed) >= SAVE_AFTER
        {
            let dashboard = data::Dashboard::from(&*self);

            self.last_changed = None;

            return Task::perform(dashboard.save(), Message::DashboardSaved);
        }

        Task::none()
    }

    pub fn toggle_command_bar(
        &mut self,
        session: &data::Session,
        version: &Version,
        config: &Config,
        theme: &mut Theme,
    ) -> Task<Message> {
        match self.command_bar_window {
            Some(window) if window == self.focus.window => {
                // Remove theme preview
                *theme = theme.selected();

                self.close_command_bar();
                // Refocus the pane so text input gets refocused
                let Focus { window, pane } = self.focus;
                self.focus_pane(window, pane)
            }
            Some(_) => {
                *theme = theme.selected();

                self.close_command_bar();
                self.open_command_bar(session, version, config);

                Task::none()
            }
            None => {
                self.open_command_bar(session, version, config);
                Task::none()
            }
        }
    }

    fn toggle_sidebar(&mut self) {
        self.side_menu.toggle_visibility();
        self.last_changed = Some(Instant::now());
    }

    fn open_command_bar(
        &mut self,
        session: &data::Session,
        version: &Version,
        config: &Config,
    ) {
        self.command_bar_window = Some(self.focus.window);
        self.command_bar = Some(CommandBar::new(
            session,
            version,
            config,
            self.focus,
            self.buffer_resize_action(),
            self.main_window(),
        ));
    }

    fn close_command_bar(&mut self) {
        self.command_bar = None;
        self.command_bar_window = None;
    }

    fn buffer_resize_action(&self) -> data::buffer::Resize {
        let can_resize_buffer =
            self.focus.window == self.main_window() && self.panes.len() > 1;
        data::buffer::Resize::action(
            can_resize_buffer,
            self.is_pane_maximized(),
        )
    }

    fn cycle_buffers(
        &self,
        session: &data::Session,
        config: &Config,
    ) -> Vec<data::Buffer> {
        // Recency order matches the sidebar; internal buffers trail.
        session
            .conversations
            .sorted()
            .into_iter()
            .map(|conversation| {
                data::Buffer::Conversation(conversation.id.clone())
            })
            .chain(
                config
                    .sidebar
                    .internal_buffers
                    .buffers
                    .iter()
                    .map(|internal| data::Buffer::Internal(internal.into())),
            )
            .collect()
    }

    fn cycle_buffers_with_has_unread(
        &self,
        session: &data::Session,
        config: &Config,
    ) -> Vec<(data::Buffer, bool)> {
        self.cycle_buffers(session, config)
            .into_iter()
            .map(|buffer| {
                let has_unread = history::Kind::from_buffer(buffer.clone())
                    .is_some_and(|kind| self.history.has_unread(&kind));

                (buffer, has_unread)
            })
            .collect()
    }

    fn from_data(
        data: data::Dashboard,
        config: &Config,
        main_window: &Window,
    ) -> (Self, Task<Message>) {
        use pane_grid::Configuration;

        fn configuration(
            pane: data::Pane,
            history: &history::Manager,
            config: &Config,
        ) -> Configuration<Pane> {
            match pane {
                data::Pane::Split { axis, ratio, a, b } => {
                    Configuration::Split {
                        axis: match axis {
                            data::pane::Axis::Horizontal => {
                                pane_grid::Axis::Horizontal
                            }
                            data::pane::Axis::Vertical => {
                                pane_grid::Axis::Vertical
                            }
                        },
                        ratio,
                        a: Box::new(configuration(*a, history, config)),
                        b: Box::new(configuration(*b, history, config)),
                    }
                }
                data::Pane::Buffer { buffer } => {
                    Configuration::Pane(Pane::new(Buffer::from_data(
                        buffer,
                        history,
                        Size::default(),
                        config,
                    )))
                }
                data::Pane::Empty => {
                    Configuration::Pane(Pane::new(Buffer::empty()))
                }
            }
        }

        let history = history::Manager::default();

        let panes = Panes {
            main_window: main_window.id,
            main: pane_grid::State::with_configuration(configuration(
                data.pane, &history, config,
            )),
            popout: HashMap::new(),
        };

        let focus = panes
            .iter()
            // This should never fail
            .find_map(|(window, pane, state)| {
                (state.buffer.data() == data.focus_buffer)
                    .then_some(Focus { window, pane })
            })
            // But if somehow it does, we just focus the "first" pane from the main window
            .unwrap_or_else(|| {
                let (_, pane) = pane_grid::State::new(());

                Focus {
                    window: main_window.id,
                    pane,
                }
            });

        let (sidebar, sidebar_task) = Sidebar::new(data.sidebar.is_hidden());

        let mut dashboard = Self {
            panes,
            focus,
            focus_history: VecDeque::from([focus.pane]),
            side_menu: sidebar,
            history,
            last_changed: None,
            command_bar: None,
            command_bar_window: None,
            theme_editor: None,
            buffer_settings: data.buffer_settings.clone(),
            status_bar: status_bar::StatusBar::default(),
            member_add_explained: data.member_add_explained,
            requested_member_loads: HashSet::new(),
        };

        let mut tasks = vec![sidebar_task.map(Message::Sidebar)];

        for pane in data.popout_panes {
            // Popouts are only a single pane
            let Configuration::Pane(pane) =
                configuration(pane, &dashboard.history, config)
            else {
                continue;
            };

            if let Some(buffer) = pane.buffer.data() {
                tasks.push(dashboard.open_buffer(
                    buffer,
                    BufferAction::NewWindow,
                    &mut stream::Map::default(),
                    config,
                ));
            }
        }

        let tasks = Task::batch(tasks)
            .chain(dashboard.focus_pane(focus.window, focus.pane));

        (dashboard, tasks)
    }

    pub fn history(&self) -> &history::Manager {
        &self.history
    }

    /// Ensures a (possibly partial) history entry exists so unread state
    /// accrues for conversations without an open pane.
    pub fn open_history(&mut self, kind: history::Kind) {
        self.history.open(kind);
    }

    pub fn find_window_with_history(
        &self,
        kind: &history::Kind,
    ) -> Option<window::Id> {
        self.panes.iter().find_map(|(window_id, _, state)| {
            state
                .buffer
                .data()
                .and_then(history::Kind::from_buffer)
                .is_some_and(|pane_kind| pane_kind == *kind)
                .then_some(window_id)
        })
    }

    pub fn handle_notification_event(
        &mut self,
        event: notification::Event,
        backend: &mut stream::Map,
        config: &Config,
    ) -> Task<Message> {
        match event {
            notification::Event::NotificationResponse { action, buffer } => {
                // When an notification action is performed in Wayland the
                // application is not automatically brought forward.  Since
                // there is currently no interface to ensure the application is
                // activated with non-dismissal notification interactions,
                // request attention in order to do so (should be a noop in
                // other environments).

                let window_id = if let Some(buffer) = &buffer
                    && let Some((window, _, _)) =
                        self.panes.get_by_buffer(buffer)
                {
                    window
                } else {
                    self.focus.window
                };

                let activate_application = iced::window::request_user_attention(
                    window_id,
                    Some(iced::window::UserAttention::Informational),
                );

                match action {
                    toast::Action::Dismiss => Task::none(),
                    toast::Action::ActivateApplication => activate_application,
                    toast::Action::OpenBuffer => {
                        if let Some(buffer) = buffer {
                            activate_application.chain(self.open_buffer(
                                buffer,
                                config.actions.notification.open_buffer,
                                backend,
                                config,
                            ))
                        } else {
                            activate_application
                        }
                    }
                }
            }
            notification::Event::RequestAttention { buffer } => {
                let window_id = if let Some(buffer) = buffer
                    && let Some((window, _, _)) =
                        self.panes.get_by_buffer(&buffer)
                {
                    window
                } else {
                    self.focus.window
                };

                iced::window::request_user_attention(
                    window_id,
                    Some(iced::window::UserAttention::Informational),
                )
            }
        }
    }

    pub fn handle_window_event(
        &mut self,
        id: window::Id,
        event: window::Event,
        theme: &mut Theme,
    ) -> Task<Message> {
        if self.panes.popout.contains_key(&id) {
            match event {
                window::Event::CloseRequested => {
                    if self.command_bar_window == Some(id) {
                        *theme = theme.selected();
                        self.close_command_bar();
                    }

                    self.panes.popout.remove(&id);
                    return window::close(id);
                }
                window::Event::Focused => {
                    return self.focus_window_pane(id);
                }
                window::Event::Moved(_)
                | window::Event::Resized(_)
                | window::Event::Unfocused
                | window::Event::Opened { .. } => {}
                window::Event::FileHovered
                | window::Event::FilesHoveredLeft
                | window::Event::FileDropped(_) => {}
            }
        } else if self.theme_editor.as_ref().is_some_and(|e| e.window == id) {
            match event {
                window::Event::CloseRequested => {
                    return self.close_theme_editor(theme);
                }
                window::Event::Moved(_)
                | window::Event::Resized(_)
                | window::Event::Focused
                | window::Event::Unfocused
                | window::Event::Opened { .. }
                | window::Event::FileHovered
                | window::Event::FilesHoveredLeft
                | window::Event::FileDropped(_) => {}
            }
        }

        Task::none()
    }

    pub fn preview_theme_in_editor(
        &mut self,
        styles: theme::Styles,
        main_window: &Window,
        theme: &mut Theme,
        config: &Config,
    ) -> Task<Message> {
        *theme = theme.preview(data::Theme::new("Custom Theme".into(), styles));

        if let Some(editor) = &self.theme_editor {
            window::gain_focus(editor.window)
        } else {
            let (editor, task) = ThemeEditor::open(main_window, config);

            self.theme_editor = Some(editor);

            task.then(|_| Task::none())
        }
    }

    /// Marks buffers as read per config, saves the dashboard, then asks
    /// the caller (via `Event::Exit`) to quit the backend and the app.
    pub fn exit(&mut self, config: &Config) -> Task<Message> {
        if config.buffer.mark_as_read.on_application_exit {
            self.history.kinds()
        } else {
            self.panes
                .iter()
                .filter_map(|(_, _, state)| {
                    if config
                        .buffer
                        .mark_as_read
                        .on_buffer_close
                        .mark_as_read(state.buffer.is_scrolled_to_bottom())
                    {
                        state.buffer.data().and_then(history::Kind::from_buffer)
                    } else {
                        None
                    }
                })
                .collect()
        }
        .into_iter()
        .for_each(|kind| {
            self.history.mark_as_read(&kind);
        });

        self.last_changed = None;
        let dashboard = data::Dashboard::from(&*self);

        Task::perform(dashboard.save(), Message::Exited)
    }

    fn main_window(&self) -> window::Id {
        self.panes.main_window
    }

    pub fn focused_buffer_name(
        &self,
        w: window::Id,
        session: &data::Session,
    ) -> Option<String> {
        let name = |pane: &Pane| -> Option<String> {
            match &pane.buffer {
                Buffer::Empty => None,
                Buffer::Conversation(state) => Some(
                    session.conversations.get(&state.convo_id).map_or_else(
                        || {
                            format!(
                                "Conversation {}",
                                state.convo_id.short_label()
                            )
                        },
                        data::Conversation::display_name,
                    ),
                ),
                Buffer::Logs(_) => Some("Logs".to_string()),
                Buffer::ConfigEditor(_) => Some("Config Editor".to_string()),
            }
        };

        if w == self.main_window() {
            self.focus_history
                .front()
                .and_then(|pane| self.panes.get(w, *pane))
                .and_then(name)
        } else {
            self.panes
                .iter()
                .find_map(|(win, _, pane)| (win == w).then(|| name(pane)))
                .flatten()
        }
    }
}

/// The wire-side conversation id for backend controls.
pub fn wire_id(convo_id: &ConvoId) -> stream::ConvoId {
    stream::ConvoId(convo_id.as_str().to_owned())
}

fn mark_as_read_on_buffer_close(
    buffer: &Buffer,
    history: &mut history::Manager,
    config: &Config,
) {
    if config
        .buffer
        .mark_as_read
        .on_buffer_close
        .mark_as_read(buffer.is_scrolled_to_bottom())
        && let Some(kind) = buffer.data().and_then(history::Kind::from_buffer)
    {
        history.mark_as_read(&kind);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Focus {
    pub window: window::Id,
    pub pane: pane_grid::Pane,
}

impl<'a> From<&'a Dashboard> for data::Dashboard {
    fn from(dashboard: &'a Dashboard) -> Self {
        use pane_grid::Node;

        fn from_layout(
            panes: &pane_grid::State<Pane>,
            node: pane_grid::Node,
        ) -> data::Pane {
            match node {
                Node::Split {
                    axis, ratio, a, b, ..
                } => data::Pane::Split {
                    axis: match axis {
                        pane_grid::Axis::Horizontal => {
                            data::pane::Axis::Horizontal
                        }
                        pane_grid::Axis::Vertical => data::pane::Axis::Vertical,
                    },
                    ratio,
                    a: Box::new(from_layout(panes, *a)),
                    b: Box::new(from_layout(panes, *b)),
                },
                Node::Pane(pane) => panes
                    .get(pane)
                    .cloned()
                    .map_or(data::Pane::Empty, data::Pane::from),
            }
        }

        let layout = dashboard.panes.main.layout().clone();
        let focus = dashboard.focus;

        data::Dashboard {
            pane: from_layout(&dashboard.panes.main, layout),
            popout_panes: dashboard
                .panes
                .popout
                .values()
                .map(|state| from_layout(state, state.layout().clone()))
                .collect(),
            buffer_settings: dashboard.buffer_settings.clone(),
            focus_buffer: dashboard.panes.iter().find_map(|(w, p, state)| {
                (w == focus.window && p == focus.pane)
                    .then_some(state.buffer.data())
                    .flatten()
            }),
            sidebar: if dashboard.side_menu.hidden {
                data::dashboard::Sidebar::Hidden
            } else {
                data::dashboard::Sidebar::Visible
            },
            member_add_explained: dashboard.member_add_explained,
        }
    }
}

#[derive(Clone)]
pub struct Panes {
    main_window: window::Id,
    main: pane_grid::State<Pane>,
    popout: HashMap<window::Id, pane_grid::State<Pane>>,
}

impl Panes {
    fn len(&self) -> usize {
        self.main.panes.len() + self.popout.len()
    }

    fn get(&self, window: window::Id, pane: pane_grid::Pane) -> Option<&Pane> {
        if self.main_window == window {
            self.main.get(pane)
        } else {
            self.popout.get(&window).and_then(|panes| panes.get(pane))
        }
    }

    fn get_mut(
        &mut self,
        window: window::Id,
        pane: pane_grid::Pane,
    ) -> Option<&mut Pane> {
        if self.main_window == window {
            self.main.get_mut(pane)
        } else {
            self.popout
                .get_mut(&window)
                .and_then(|panes| panes.get_mut(pane))
        }
    }

    fn get_by_buffer(
        &self,
        buffer: &data::Buffer,
    ) -> Option<(window::Id, pane_grid::Pane, &Pane)> {
        self.iter().find(|(_, _, state)| {
            state.buffer.data().is_some_and(|b| b == *buffer)
        })
    }

    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (window::Id, pane_grid::Pane, &Pane)> {
        self.main
            .iter()
            .map(move |(pane, state)| (self.main_window, *pane, state))
            .chain(self.popout.iter().flat_map(|(window_id, panes)| {
                panes.iter().map(|(pane, state)| (*window_id, *pane, state))
            }))
    }

    fn iter_mut(
        &mut self,
    ) -> impl Iterator<Item = (window::Id, pane_grid::Pane, &mut Pane)> {
        let main_window = self.main_window;

        self.main
            .iter_mut()
            .map(move |(pane, state)| (main_window, *pane, state))
            .chain(self.popout.iter_mut().flat_map(|(window_id, panes)| {
                panes
                    .iter_mut()
                    .map(|(pane, state)| (*window_id, *pane, state))
            }))
    }

    fn resources(&self) -> impl Iterator<Item = data::history::Resource> + '_ {
        self.main.panes.values().filter_map(Pane::resource).chain(
            self.popout.values().flat_map(|state| {
                state.panes.values().filter_map(Pane::resource)
            }),
        )
    }
}

/// Every buffer the command bar / cycling can target: all conversations in
/// recency order plus the internal buffers.
fn open_buffers(dashboard: &Dashboard) -> Vec<data::Buffer> {
    dashboard
        .panes
        .iter()
        .filter_map(|(_, _, pane)| pane.buffer.data())
        .collect()
}

fn cycle_next_buffer(
    current: Option<&data::Buffer>,
    mut all: Vec<data::Buffer>,
    opened: &[data::Buffer],
) -> Option<data::Buffer> {
    all.retain(|buffer| Some(buffer) == current || !opened.contains(buffer));

    let next = || {
        let buffer = current?;
        let index = all.iter().position(|b| b == buffer)?;
        all.get(index + 1)
    };

    next().or_else(|| all.first()).cloned()
}

fn cycle_previous_buffer(
    current: Option<&data::Buffer>,
    mut all: Vec<data::Buffer>,
    opened: &[data::Buffer],
) -> Option<data::Buffer> {
    all.retain(|buffer| Some(buffer) == current || !opened.contains(buffer));

    let previous = || {
        let buffer = current?;
        let index = all.iter().position(|b| b == buffer).filter(|i| *i > 0)?;

        all.get(index - 1)
    };

    previous().or_else(|| all.last()).cloned()
}

fn cycle_next_unread_buffer(
    current: Option<&data::Buffer>,
    mut all: Vec<(data::Buffer, bool)>,
    opened: &[data::Buffer],
) -> Option<data::Buffer> {
    all.retain(|(buffer, _)| {
        Some(buffer) == current || !opened.contains(buffer)
    });

    let index = current
        .and_then(|buffer| all.iter().position(|(b, _)| b == buffer))
        .unwrap_or(all.len());

    let next_after = || {
        all.iter()
            .skip(index + 1)
            .find_map(|(b, has_unread)| has_unread.then_some(b))
    };

    let next_before = || {
        all.iter()
            .take(index)
            .find_map(|(b, has_unread)| has_unread.then_some(b))
    };

    next_after().or_else(|| next_before().or(None)).cloned()
}

fn cycle_previous_unread_buffer(
    current: Option<&data::Buffer>,
    mut all: Vec<(data::Buffer, bool)>,
    opened: &[data::Buffer],
) -> Option<data::Buffer> {
    all.retain(|(buffer, _)| {
        Some(buffer) == current || !opened.contains(buffer)
    });

    let index = current
        .and_then(|buffer| all.iter().rev().position(|(b, _)| b == buffer))
        .unwrap_or(all.len());

    let previous_before = || {
        all.iter()
            .rev()
            .skip(index + 1)
            .find_map(|(b, has_unread)| has_unread.then_some(b))
    };

    let previous_after = || {
        all.iter()
            .rev()
            .take(index)
            .find_map(|(b, has_unread)| has_unread.then_some(b))
    };

    previous_before()
        .or_else(|| previous_after().or(None))
        .cloned()
}
