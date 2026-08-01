use data::conversation::Kind;
use data::{Config, history};
use iced::Size;
use iced::widget::text::Wrapping;
use iced::widget::{button, center, column, pane_grid, row, sensor, text};

use super::sidebar;
use crate::buffer::{self, Buffer};
use crate::widget::{Element, tooltip};
use crate::{Theme, icon, theme, widget};

#[derive(Debug, Clone)]
pub enum Message {
    PaneClicked(pane_grid::Pane),
    PaneResized(pane_grid::ResizeEvent),
    PaneDragged(pane_grid::DragEvent),
    Buffer(pane_grid::Pane, buffer::Message),
    ClosePane,
    SplitPane(pane_grid::Axis),
    MaximizePane,
    ToggleShowMemberList,
    ToggleShowDetails,
    Popout,
    Merge,
    ScrollToBottom,
    MarkAsRead,
    ClearBuffer,
    ContentResized(pane_grid::Pane, Size),
}

#[derive(Clone, Debug)]
pub struct Pane {
    pub buffer: Buffer,
    pub size: Size,
    title_bar: TitleBar,
}

#[derive(Debug, Clone, Default)]
pub struct TitleBar {}

impl Pane {
    pub fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            size: Size::default(), // Will get set initially via `Message::Resized`
            title_bar: TitleBar::default(),
        }
    }

    pub fn view<'a>(
        &'a self,
        id: pane_grid::Pane,
        panes: usize,
        is_focused: bool,
        maximized: bool,
        session: &'a data::Session,
        history: &'a history::Manager,
        sidebar: &'a sidebar::Sidebar,
        config: &'a Config,
        theme: &'a Theme,
        settings: Option<&'a data::buffer::Settings>,
        is_popout: bool,
    ) -> widget::Content<'a, Message> {
        let title: Element<'a, Message> = match &self.buffer {
            Buffer::Empty => text("").into(),
            Buffer::Conversation(state) => {
                let conversation = session.conversations.get(&state.convo_id);

                let display_name = conversation.map_or_else(
                    || format!("Conversation {}", state.convo_id.short_label()),
                    data::Conversation::display_name,
                );

                let member_count = conversation.and_then(|conversation| {
                    (conversation.kind == Kind::Group
                        && !conversation.members.is_empty())
                    .then(|| {
                        format!(
                            " · {} members",
                            conversation.joined_member_count()
                        )
                    })
                });

                row![
                    text(display_name)
                        .style(theme::text::primary)
                        .wrapping(Wrapping::None)
                        .ellipsis(text::Ellipsis::End),
                    member_count.map(|member_count| {
                        text(member_count)
                            .style(theme::text::secondary)
                            .wrapping(Wrapping::None)
                            .ellipsis(text::Ellipsis::End)
                    }),
                ]
                .into()
            }
            Buffer::Module(state) => {
                let module = session.module(&state.module);

                // The same primary/secondary pair the conversation title
                // uses for its member count: what it is, then what it is
                // doing. The status is the fact the pane exists to carry, so
                // it is never omitted once the daemon has reported.
                let status = module.map(|module| {
                    let version = module
                        .version
                        .as_deref()
                        .map(|version| format!(" · {version}"))
                        .unwrap_or_default();

                    format!(" · {}{version}", module.status.label())
                });

                row![
                    text(state.module.display_name())
                        .style(theme::text::primary)
                        .wrapping(Wrapping::None)
                        .ellipsis(text::Ellipsis::End),
                    status.map(|status| {
                        text(status)
                            .style(
                                if module.map(|module| &module.status)
                                    == Some(&data::module::Status::Crashed)
                                {
                                    theme::text::error
                                } else {
                                    theme::text::secondary
                                },
                            )
                            .wrapping(Wrapping::None)
                            .ellipsis(text::Ellipsis::End)
                    }),
                ]
                .into()
            }
            Buffer::Logs(_) => text("Logs")
                .wrapping(Wrapping::None)
                .ellipsis(text::Ellipsis::End)
                .into(),
            Buffer::ConfigEditor(_) => text("Config Editor")
                .wrapping(Wrapping::None)
                .ellipsis(text::Ellipsis::End)
                .into(),
        };

        let title_bar = self.title_bar.view(
            &self.buffer,
            session,
            history,
            title,
            id,
            panes,
            maximized,
            settings,
            !config.pane.always_show_title_bar_buttons,
            false,
            config.tooltips.show_for_buttons(),
            is_popout,
            config,
            theme,
        );

        let content = self
            .buffer
            .view(
                session, history, settings, config, theme, is_focused, sidebar,
            )
            .map(move |msg| Message::Buffer(id, msg));

        let content = sensor(content)
            .on_resize(move |size| Message::ContentResized(id, size));

        let content: Element<'a, Message> = column![content].into();

        widget::Content::new(content)
            .style(move |theme| theme::container::buffer(theme, is_focused))
            .title_bar(title_bar.style(theme::container::buffer_title_bar))
    }

    pub fn resource(&self) -> Option<history::Resource> {
        match &self.buffer {
            Buffer::Empty => None,
            Buffer::Conversation(state) => Some(history::Resource {
                kind: history::Kind::Conversation(state.convo_id.clone()),
            }),
            Buffer::Module(state) => {
                Some(history::Resource::module(state.module.clone()))
            }
            Buffer::Logs(_) => Some(history::Resource::logs()),
            Buffer::ConfigEditor(_) => None,
        }
    }
}

impl TitleBar {
    fn view<'a>(
        &'a self,
        buffer: &Buffer,
        session: &'a data::Session,
        history: &'a history::Manager,
        title: Element<'a, Message>,
        id: pane_grid::Pane,
        panes: usize,
        maximized: bool,
        settings: Option<&'a data::buffer::Settings>,
        only_show_controls_on_hover: bool,
        hide_controls: bool,
        show_tooltips: bool,
        is_popout: bool,
        config: &'a Config,
        theme: &'a Theme,
    ) -> widget::TitleBar<'a, Message> {
        let maybe_buffer_kind =
            buffer.data().and_then(history::Kind::from_buffer);
        let can_mark_as_read = if let Some(kind) = &maybe_buffer_kind {
            history.can_mark_as_read(kind)
        } else {
            false
        };
        let has_unread = if let Some(kind) = &maybe_buffer_kind {
            history.has_unread(kind)
        } else {
            false
        };

        let is_group = buffer.convo_id().is_some_and(|convo_id| {
            session
                .conversations
                .get(convo_id)
                .is_some_and(|conversation| conversation.kind == Kind::Group)
        });

        // Pane controls.
        let controls = row![
            if let Buffer::ConfigEditor(state) = &buffer {
                let is_dirty = state.has_unsaved_changes();

                let save_button = button(center(icon::checkmark()))
                    .padding(5)
                    .width(22)
                    .height(22)
                    .on_press_maybe(is_dirty.then_some(Message::Buffer(
                        id,
                        buffer::Message::ConfigEditor(
                            buffer::config_editor::Message::Save,
                        ),
                    )))
                    .style(move |theme, status| {
                        theme::button::secondary(theme, status, is_dirty)
                    });

                let save_button_with_tooltip = tooltip(
                    save_button,
                    show_tooltips.then(|| {
                        save_config_tooltip(
                            is_dirty,
                            &config.keyboard.config_editor_save,
                        )
                    }),
                    tooltip::Position::Bottom,
                    theme,
                );
                Some(save_button_with_tooltip)
            } else {
                None
            },
            if matches!(buffer, Buffer::ConfigEditor(_)) {
                let reload_button = button(center(icon::refresh()))
                    .padding(5)
                    .width(22)
                    .height(22)
                    .on_press(Message::Buffer(
                        id,
                        buffer::Message::ConfigEditor(
                            buffer::config_editor::Message::Refresh,
                        ),
                    ))
                    .style(|theme, status| {
                        theme::button::secondary(theme, status, false)
                    });

                let reload_button_with_tooltip = tooltip(
                    reload_button,
                    show_tooltips.then_some(
                        match config.keyboard.reload_configuration.primary() {
                            Some(
                                keybind @ data::shortcut::KeyBind::Bind {
                                    ..
                                },
                            ) => {
                                format!("Reload file from disk ({keybind})")
                            }
                            _ => "Reload file from disk".to_string(),
                        },
                    ),
                    tooltip::Position::Bottom,
                    theme,
                );
                Some(reload_button_with_tooltip)
            } else {
                None
            },
            if matches!(buffer, Buffer::ConfigEditor(_)) {
                let open_directory_button = button(center(icon::open()))
                    .padding(5)
                    .width(22)
                    .height(22)
                    .on_press(Message::Buffer(
                        id,
                        buffer::Message::ConfigEditor(
                            buffer::config_editor::Message::OpenDirectory,
                        ),
                    ))
                    .style(|theme, status| {
                        theme::button::secondary(theme, status, false)
                    });

                let open_directory_button_with_tooltip = tooltip(
                    open_directory_button,
                    show_tooltips.then_some("Open config directory"),
                    tooltip::Position::Bottom,
                    theme,
                );
                Some(open_directory_button_with_tooltip)
            } else {
                None
            },
            if matches!(buffer, Buffer::ConfigEditor(_)) {
                let open_config_file_button = button(center(icon::config()))
                    .padding(5)
                    .width(22)
                    .height(22)
                    .on_press(Message::Buffer(
                        id,
                        buffer::Message::ConfigEditor(
                            buffer::config_editor::Message::OpenConfigFile,
                        ),
                    ))
                    .style(|theme, status| {
                        theme::button::secondary(theme, status, false)
                    });

                let open_config_file_button_with_tooltip = tooltip(
                    open_config_file_button,
                    show_tooltips.then_some(
                        match config.keyboard.open_config_file.primary() {
                            Some(
                                keybind @ data::shortcut::KeyBind::Bind {
                                    ..
                                },
                            ) => {
                                format!("Open config file ({keybind})")
                            }
                            _ => "Open config file".to_string(),
                        },
                    ),
                    tooltip::Position::Bottom,
                    theme,
                );
                Some(open_config_file_button_with_tooltip)
            } else {
                None
            },
            if maybe_buffer_kind.is_some() {
                let mark_as_read_button = button(center(icon::mark_as_read()))
                    .padding(5)
                    .width(22)
                    .height(22)
                    .on_press_maybe(
                        can_mark_as_read.then_some(Message::MarkAsRead),
                    )
                    .style(move |theme, status| {
                        theme::button::secondary(theme, status, has_unread)
                    });

                let mark_as_read_button_with_tooltip = tooltip(
                    mark_as_read_button,
                    show_tooltips.then_some(if can_mark_as_read {
                        match config.keyboard.mark_as_read.primary() {
                            Some(
                                keybind @ data::shortcut::KeyBind::Bind {
                                    ..
                                },
                            ) => {
                                format!("Mark messages as read ({keybind})")
                            }
                            _ => "Mark messages as read".to_string(),
                        }
                    } else {
                        "No unread messages".to_string()
                    }),
                    tooltip::Position::Bottom,
                    theme,
                );
                Some(mark_as_read_button_with_tooltip)
            } else {
                None
            },
            {
                if maybe_buffer_kind.is_some() {
                    let can_scroll_to_bottom =
                        !buffer.is_scrolled_to_bottom().unwrap_or_default();
                    let scroll_to_bottom_button =
                        button(center(icon::scroll_to_bottom()))
                            .padding(5)
                            .width(22)
                            .height(22)
                            .on_press_maybe(
                                can_scroll_to_bottom
                                    .then_some(Message::ScrollToBottom),
                            )
                            .style(|theme, status| {
                                theme::button::secondary(theme, status, false)
                            });

                    let scroll_to_bottom_button_with_tooltip = tooltip(
                        scroll_to_bottom_button,
                        show_tooltips.then_some(if can_scroll_to_bottom {
                            match config.keyboard.scroll_to_bottom.primary() {
                                Some(
                                    keybind @ data::shortcut::KeyBind::Bind {
                                        ..
                                    },
                                ) => {
                                    format!("Scroll to bottom ({keybind})")
                                }
                                _ => "Scroll to bottom".to_string(),
                            }
                        } else {
                            "Already at bottom".to_string()
                        }),
                        tooltip::Position::Bottom,
                        theme,
                    );
                    Some(scroll_to_bottom_button_with_tooltip)
                } else {
                    None
                }
            },
            {
                if maybe_buffer_kind.is_some() {
                    let clear_history_button = button(center(icon::eraser()))
                        .padding(5)
                        .width(22)
                        .height(22)
                        .on_press(Message::ClearBuffer)
                        .style(|theme, status| {
                            theme::button::secondary(theme, status, false)
                        });

                    let clear_history_button_with_tooltip = tooltip(
                        clear_history_button,
                        show_tooltips.then_some("Clear buffer"),
                        tooltip::Position::Bottom,
                        theme,
                    );
                    Some(clear_history_button_with_tooltip)
                } else {
                    None
                }
            },
            if let Buffer::Conversation(state) = &buffer {
                let details_shown = state.show_details;

                let details_button = button(center(icon::about()))
                    .padding(5)
                    .width(22)
                    .height(22)
                    .on_press(Message::ToggleShowDetails)
                    .style(move |theme, status| {
                        theme::button::secondary(theme, status, details_shown)
                    });

                let details_button_with_tooltip = tooltip(
                    details_button,
                    show_tooltips.then_some(if details_shown {
                        "Hide details".to_string()
                    } else {
                        "Show details".to_string()
                    }),
                    tooltip::Position::Bottom,
                    theme,
                );
                Some(details_button_with_tooltip)
            } else {
                None
            },
            if is_group {
                let member_list_enabled = settings.map_or(
                    config.buffer.conversation.member_list.enabled,
                    |settings| settings.conversation.member_list.enabled,
                );

                let member_list_button = button(center(icon::people()))
                    .padding(5)
                    .width(22)
                    .height(22)
                    .on_press(Message::ToggleShowMemberList)
                    .style(move |theme, status| {
                        theme::button::secondary(
                            theme,
                            status,
                            member_list_enabled,
                        )
                    });

                let member_list_button_with_tooltip = tooltip(
                    member_list_button,
                    show_tooltips.then_some(if member_list_enabled {
                        "Hide member list".to_string()
                    } else {
                        "Show member list".to_string()
                    }),
                    tooltip::Position::Bottom,
                    theme,
                );
                Some(member_list_button_with_tooltip)
            } else {
                None
            },
            if panes > 1 {
                let maximize_button = button(center(if maximized {
                    icon::restore()
                } else {
                    icon::maximize()
                }))
                .padding(5)
                .width(22)
                .height(22)
                .on_press(Message::MaximizePane)
                .style(move |theme, status| {
                    theme::button::secondary(theme, status, maximized)
                });

                let maximize_button_with_tooltip = tooltip(
                    maximize_button,
                    show_tooltips.then_some(if maximized {
                        match config.keyboard.restore_buffer.primary() {
                            Some(
                                keybind @ data::shortcut::KeyBind::Bind {
                                    ..
                                },
                            ) => {
                                format!("Restore ({keybind})")
                            }
                            _ => "Restore".to_string(),
                        }
                    } else {
                        match config.keyboard.maximize_buffer.primary() {
                            Some(
                                keybind @ data::shortcut::KeyBind::Bind {
                                    ..
                                },
                            ) => {
                                format!("Maximize ({keybind})")
                            }
                            _ => "Maximize".to_string(),
                        }
                    }),
                    tooltip::Position::Bottom,
                    theme,
                );
                Some(maximize_button_with_tooltip)
            } else {
                None
            },
            if is_popout {
                let merge_button = button(center(icon::popout()))
                    .padding(5)
                    .width(22)
                    .height(22)
                    .on_press(Message::Merge)
                    .style(|theme, status| {
                        theme::button::secondary(theme, status, true)
                    });

                let merge_button_with_tooltip = tooltip(
                    merge_button,
                    show_tooltips.then_some("Merge"),
                    tooltip::Position::Bottom,
                    theme,
                );
                Some(merge_button_with_tooltip)
            } else if panes > 1 {
                let popout_button = button(center(icon::popout()))
                    .padding(5)
                    .width(22)
                    .height(22)
                    .on_press(Message::Popout)
                    .style(|theme, status| {
                        theme::button::secondary(theme, status, false)
                    });

                let popout_button_with_tooltip = tooltip(
                    popout_button,
                    show_tooltips.then_some("Pop out"),
                    tooltip::Position::Bottom,
                    theme,
                );
                Some(popout_button_with_tooltip)
            } else {
                None
            },
            if !(is_popout || panes == 1 && matches!(buffer, Buffer::Empty)) {
                let close_button = button(center(icon::cancel()))
                    .padding(5)
                    .width(22)
                    .height(22)
                    .on_press(Message::ClosePane)
                    .style(|theme, status| {
                        theme::button::secondary(theme, status, false)
                    });

                let close_button_with_tooltip = tooltip(
                    close_button,
                    show_tooltips.then_some(
                        match config.keyboard.close_buffer.primary() {
                            Some(
                                keybind @ data::shortcut::KeyBind::Bind {
                                    ..
                                },
                            ) => {
                                format!("Close ({keybind})")
                            }
                            _ => "Close".to_string(),
                        },
                    ),
                    tooltip::Position::Bottom,
                    theme,
                );
                Some(close_button_with_tooltip)
            } else {
                None
            },
        ]
        .spacing(2);

        let title = iced::widget::container(title)
            .height(theme::resolve_line_height(&config.font).ceil().max(22.0))
            .padding([0, 4])
            .align_y(iced::alignment::Vertical::Center);

        let mut title_bar = widget::TitleBar::new(title).padding(6);

        if !only_show_controls_on_hover {
            title_bar = title_bar.always_show_controls();
        }

        if hide_controls {
            title_bar
        } else {
            title_bar.controls(pane_grid::Controls::new(controls))
        }
    }
}

fn save_config_tooltip(
    is_dirty: bool,
    keybinds: &data::shortcut::KeyBinds,
) -> String {
    if !is_dirty {
        return "No unsaved changes".to_string();
    }

    match keybinds.primary() {
        Some(keybind @ data::shortcut::KeyBind::Bind { .. }) => {
            format!("Save and reload config ({keybind})")
        }
        _ => "Save and reload config".to_string(),
    }
}

impl From<Pane> for data::Pane {
    fn from(pane: Pane) -> Self {
        let buffer = match pane.buffer {
            Buffer::Empty => return data::Pane::Empty,
            Buffer::Conversation(state) => {
                data::Buffer::Conversation(state.convo_id)
            }
            Buffer::Module(state) => data::Buffer::Module(state.module),
            Buffer::Logs(_) => {
                data::Buffer::Internal(data::buffer::Internal::Logs)
            }
            Buffer::ConfigEditor(_) => {
                data::Buffer::Internal(data::buffer::Internal::ConfigEditor)
            }
        };

        data::Pane::Buffer { buffer }
    }
}
