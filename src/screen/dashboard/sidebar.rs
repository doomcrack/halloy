use std::time::Duration;

use data::appearance::theme::FontStyle;
use data::config::{self, Config, sidebar};
use data::conversation::{ConvoId, Kind};
use data::dashboard::{BufferAction, BufferFocusedAction};
use data::{Version, buffer, history};
use iced::Length::Shrink;
use iced::widget::text::{Ellipsis, LineHeight, Shaping, Wrapping};
use iced::widget::{
    Column, Row, Scrollable, Space, button, column, container, pane_grid, row,
    rule, scrollable, space,
};
use iced::{
    Alignment, Border, Length, Padding, Task, clipboard, mouse, padding,
};
use tokio::time;

use super::account_card::{self, AccountCard};
use super::{Focus, Panes};
use crate::widget::{
    Element, TextExt, avatar, context_menu, double_pass, text,
};
use crate::{Theme, font, icon, platform_specific, theme, window};

const CONFIG_RELOAD_DELAY: Duration = Duration::from_secs(1);

const AVATAR_SIZE: f32 = 26.0;

/// Cap on preview characters fed to a row; keeps the measuring pass of
/// the double-width layout from stretching the sidebar to a whole
/// 160-char preview.
const PREVIEW_DISPLAY_CHARS: usize = 32;

#[derive(Debug, Clone)]
pub enum Message {
    New(data::Buffer),
    Popout(data::Buffer),
    Focus(window::Id, pane_grid::Pane),
    Replace(data::Buffer),
    Close(window::Id, pane_grid::Pane),
    Swap(window::Id, pane_grid::Pane),
    ToggleCommandBar,
    ToggleThemeEditor,
    ReloadConfigFile,
    ConfigReloaded(Result<Config, config::Error>),
    OpenReleaseWebsite,
    OpenAbout {
        version: String,
        commit: String,
        system_information: Option<iced::system::Information>,
    },
    ReloadComplete,
    MarkAsRead(data::Buffer),
    QuitApplication,
    SystemInformation(iced::system::Information),
    OpenNewDm,
    OpenNewGroup,
    CopyText(String),
    OpenSetNickname(ConvoId),
    ConfirmDelete(ConvoId),
    CopyAddress(String),
    CopyAddressFlashEnded(u64),
}

#[derive(Debug, Clone)]
pub enum Event {
    New(data::Buffer),
    Popout(data::Buffer),
    Focus(window::Id, pane_grid::Pane),
    Replace(data::Buffer),
    Close(window::Id, pane_grid::Pane),
    Swap(window::Id, pane_grid::Pane),
    ToggleCommandBar,
    ToggleThemeEditor,
    OpenReleaseWebsite,
    OpenAbout {
        version: String,
        commit: String,
        system_information: Option<iced::system::Information>,
    },
    ConfigReloaded(Result<Config, config::Error>),
    MarkAsRead(data::Buffer),
    QuitApplication,
    OpenNewDm,
    OpenNewGroup,
    OpenSetNickname(ConvoId),
    ConfirmDeleteConversation(ConvoId),
}

#[derive(Clone)]
pub struct Sidebar {
    pub hidden: bool,
    reloading_config: bool,
    system_information: Option<iced::system::Information>,
    account_card: AccountCard,
}

impl Sidebar {
    pub fn new(hidden: bool) -> (Self, Task<Message>) {
        (
            Self {
                hidden,
                reloading_config: false,
                system_information: None,
                account_card: AccountCard::default(),
            },
            iced::system::information().map(Message::SystemInformation),
        )
    }

    pub fn toggle_visibility(&mut self) {
        self.hidden = !self.hidden;
    }

    pub fn update(
        &mut self,
        message: Message,
    ) -> (Task<Message>, Option<Event>) {
        match message {
            Message::SystemInformation(information) => {
                self.system_information = Some(information);
                (Task::none(), None)
            }
            Message::QuitApplication => {
                (Task::none(), Some(Event::QuitApplication))
            }
            Message::New(source) => (Task::none(), Some(Event::New(source))),
            Message::Popout(source) => {
                (Task::none(), Some(Event::Popout(source)))
            }
            Message::Focus(window, pane) => {
                (Task::none(), Some(Event::Focus(window, pane)))
            }
            Message::Replace(source) => {
                (Task::none(), Some(Event::Replace(source)))
            }
            Message::Close(window, pane) => {
                (Task::none(), Some(Event::Close(window, pane)))
            }
            Message::Swap(window, pane) => {
                (Task::none(), Some(Event::Swap(window, pane)))
            }
            Message::ToggleCommandBar => {
                (Task::none(), Some(Event::ToggleCommandBar))
            }
            Message::ToggleThemeEditor => {
                (Task::none(), Some(Event::ToggleThemeEditor))
            }
            Message::ReloadConfigFile => {
                self.reloading_config = true;
                (Task::perform(Config::load(), Message::ConfigReloaded), None)
            }
            Message::ConfigReloaded(config) => (
                Task::perform(time::sleep(CONFIG_RELOAD_DELAY), |()| {
                    Message::ReloadComplete
                }),
                Some(Event::ConfigReloaded(config)),
            ),
            Message::OpenReleaseWebsite => {
                (Task::none(), Some(Event::OpenReleaseWebsite))
            }
            Message::ReloadComplete => {
                self.reloading_config = false;
                (Task::none(), None)
            }
            Message::MarkAsRead(buffer) => {
                (Task::none(), Some(Event::MarkAsRead(buffer)))
            }
            Message::OpenAbout {
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
            Message::OpenNewDm => (Task::none(), Some(Event::OpenNewDm)),
            Message::OpenNewGroup => (Task::none(), Some(Event::OpenNewGroup)),
            Message::CopyText(text) => (clipboard::write(text).discard(), None),
            Message::OpenSetNickname(convo_id) => {
                (Task::none(), Some(Event::OpenSetNickname(convo_id)))
            }
            Message::ConfirmDelete(convo_id) => (
                Task::none(),
                Some(Event::ConfirmDeleteConversation(convo_id)),
            ),
            Message::CopyAddress(address) => {
                let generation = self.account_card.copy_started();

                (
                    Task::batch(vec![
                        clipboard::write(address).discard(),
                        Task::perform(
                            time::sleep(account_card::COPY_FLASH_DURATION),
                            move |()| {
                                Message::CopyAddressFlashEnded(generation)
                            },
                        ),
                    ]),
                    None,
                )
            }
            Message::CopyAddressFlashEnded(generation) => {
                self.account_card.copy_expired(generation);
                (Task::none(), None)
            }
        }
    }

    pub fn view<'a>(
        &'a self,
        session: &'a data::Session,
        history: &'a history::Manager,
        panes: &'a Panes,
        focus: Focus,
        config: &'a Config,
        version: &'a Version,
        theme: &'a Theme,
    ) -> Option<Element<'a, Message>> {
        if self.hidden {
            return None;
        }

        let content = |width| {
            let account_card = account_card::view(
                self.account_card,
                session,
                history,
                config,
                version,
                theme,
                self.reloading_config,
                self.system_information.clone(),
                config.sidebar.position.is_horizontal(),
                width,
            );

            let new_chat = new_chat_button(
                session.delivery.can_act(),
                config,
                theme,
                width,
            );

            let mut conversations = session.conversations.sorted();

            if matches!(config.sidebar.ordering, sidebar::Ordering::Alpha) {
                conversations.sort_by_key(|conversation| {
                    conversation.display_name().to_lowercase()
                });
            }

            let conversation_rows = conversations
                .into_iter()
                .map(|conversation| {
                    conversation_button(
                        config,
                        panes,
                        focus,
                        conversation,
                        history,
                        width,
                        theme,
                    )
                })
                .collect::<Vec<_>>();

            let internal_rows = config
                .sidebar
                .internal_buffers
                .buffers
                .iter()
                .filter_map(|internal| {
                    let buffer = buffer::Internal::from(internal);

                    internal_buffer_button(
                        config, panes, focus, buffer, history, width, theme,
                    )
                })
                .collect::<Vec<_>>();

            let mut buffers: Vec<Element<'a, Message>> = vec![];

            if config.sidebar.position.is_horizontal() {
                buffers.push(space::horizontal().width(4).into());
            }

            let empty_state: Option<Element<'a, Message>> = if conversation_rows
                .is_empty()
            {
                Some(
                    container(
                        text(if session.delivery.can_act() {
                            "No conversations yet"
                        } else {
                            "Waiting for connection..."
                        })
                        .style(theme::text::secondary)
                        .font_maybe(
                            theme::font_style::secondary(theme).map(font::get),
                        ),
                    )
                    .padding(config.sidebar.padding.buffer)
                    .into(),
                )
            } else {
                None
            };

            let groups = if config
                .sidebar
                .internal_buffers
                .is_before_conversations()
            {
                [
                    internal_rows,
                    conversation_rows.into_iter().chain(empty_state).collect(),
                ]
            } else {
                [
                    conversation_rows.into_iter().chain(empty_state).collect(),
                    internal_rows,
                ]
            };

            for (index, group) in groups
                .into_iter()
                .filter(|group| !group.is_empty())
                .enumerate()
            {
                // Separator between conversations and internal buffers.
                if index > 0 {
                    if config.sidebar.position.is_horizontal() {
                        buffers.push(
                            space::horizontal()
                                .width(config.sidebar.spacing.section)
                                .into(),
                        );
                    } else {
                        buffers.push(
                            space::vertical()
                                .height(config.sidebar.spacing.section)
                                .into(),
                        );
                    }
                }

                buffers.extend(group);
            }

            match config.sidebar.position {
                sidebar::Position::Left | sidebar::Position::Right => {
                    let column_padding = if matches!(
                        config.sidebar.position,
                        sidebar::Position::Left
                    ) {
                        padding::right(2)
                    } else {
                        padding::left(2)
                    };

                    // Add buffers to a column.
                    let buffers = column![
                        Scrollable::new(
                            Column::with_children(buffers)
                                .spacing(1)
                                .padding(column_padding)
                        )
                        .direction(
                            scrollable::Direction::Vertical(
                                scrollable::Scrollbar::default()
                                    .width(config.sidebar.scrollbar.width)
                                    .scroller_width(
                                        config.sidebar.scrollbar.scroller_width
                                    )
                                    .spacing(4)
                            )
                        )
                    ];

                    // New-chat entry above, the account card at the foot.
                    let content = column![
                        container(new_chat).padding(padding::bottom(6)),
                        container(buffers).height(Length::Fill),
                        account_card,
                    ];

                    container(content)
                }
                sidebar::Position::Top | sidebar::Position::Bottom => {
                    // Add buffers to a row.
                    let buffers = row![
                        Scrollable::new(
                            Row::with_children(buffers)
                                .spacing(2)
                                .align_y(Alignment::Center)
                        )
                        .direction(
                            scrollable::Direction::Horizontal(
                                scrollable::Scrollbar::default()
                                    .width(config.sidebar.scrollbar.width)
                                    .scroller_width(
                                        config.sidebar.scrollbar.scroller_width
                                    )
                                    .spacing(4)
                            )
                        )
                    ];

                    // New-chat entry ahead, the compact account card after.
                    let content = row![
                        new_chat,
                        container(buffers).width(Length::Fill),
                        account_card,
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center);

                    container(content)
                }
            }
        };

        let platform_specific_padding =
            platform_specific::sidebar_padding(config);

        let padding = match config.sidebar.position {
            sidebar::Position::Left => {
                padding::top(8 + platform_specific_padding)
                    .bottom(6)
                    .left(6)
            }
            sidebar::Position::Right => {
                padding::top(8 + platform_specific_padding)
                    .bottom(6)
                    .right(6)
            }
            sidebar::Position::Top => {
                padding::top(8 + platform_specific_padding).right(6)
            }
            sidebar::Position::Bottom => padding::bottom(8)
                .left(6)
                .right(6)
                .top(platform_specific_padding),
        };

        let content = if config.sidebar.position.is_horizontal() {
            container(
                content(Length::Shrink).width(Length::Fill).padding(padding),
            )
        } else {
            let first_pass = content(Length::Shrink);
            let second_pass = content(Length::Fill);

            container(double_pass(first_pass, second_pass))
                .width(Shrink.max(
                    config.sidebar.max_width.map_or(f32::INFINITY, f32::from),
                ))
                .padding(padding)
        };

        Some(content.into())
    }
}

#[derive(Debug, Clone, Copy)]
enum Entry {
    Context,
    HorizontalRule,
    MarkAsRead,
    NewPane,
    Popout,
    Replace,
    Close(window::Id, pane_grid::Pane),
    Swap(window::Id, pane_grid::Pane),
    CopyConvoId,
    CopyAddress,
    SetNickname,
    Delete,
}

impl Entry {
    fn list(
        num_panes: usize,
        open_as_window_pane: Option<(window::Id, pane_grid::Pane)>,
        focus: Focus,
        has_history: bool,
        conversation_kind: Option<Kind>,
    ) -> Vec<Self> {
        use Entry::*;

        let mut entries = vec![Context, HorizontalRule];

        if has_history {
            entries.push(MarkAsRead);
        }

        match open_as_window_pane {
            None => {
                entries.extend([NewPane, Popout, Replace]);
            }
            Some((window, pane)) => {
                if num_panes > 1 {
                    entries.push(Close(window, pane));
                }
                if (Focus { window, pane }) != focus {
                    entries.push(Swap(window, pane));
                }
            }
        }

        if let Some(kind) = conversation_kind {
            entries.extend([HorizontalRule, CopyConvoId]);

            if kind == Kind::Direct {
                entries.push(CopyAddress);
            }

            entries.extend([SetNickname, HorizontalRule, Delete]);
        }

        entries
    }
}

/// The New-chat entry point (QML `ConversationsPane` parity): a menu of
/// the two conversations one can start, disabled while offline.
fn new_chat_button<'a>(
    online: bool,
    config: &'a Config,
    theme: &'a Theme,
    width: Length,
) -> Element<'a, Message> {
    // A `Fill` spacer would inflate the sidebar's measuring pass; only
    // the sized pass pushes the caret to the trailing edge.
    let spacer: Element<'a, Message> = if matches!(width, Length::Fill) {
        space::horizontal().into()
    } else {
        Space::new().width(0).into()
    };

    let label = row![
        icon::plus().size(theme::TEXT_SIZE - 3.0),
        text("New chat"),
        spacer,
        icon::chevron_down().size(theme::TEXT_SIZE - 4.0),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let base = button(label)
        .width(width)
        .padding(config.sidebar.padding.buffer)
        .style(move |theme, status| {
            // The menu wrapper owns the click, so the button carries no
            // on_press; map its Disabled look back to Active while online.
            let status = if online
                && matches!(status, iced::widget::button::Status::Disabled)
            {
                iced::widget::button::Status::Active
            } else {
                status
            };

            theme::button::primary(theme, status, false)
        });

    if !online {
        return base.into();
    }

    let entries = vec![NewChatEntry::Direct, NewChatEntry::Group];

    context_menu(
        context_menu::MouseButton::Left,
        context_menu::Anchor::Widget,
        context_menu::ToggleBehavior::Close,
        Some(mouse::Interaction::Pointer),
        base,
        entries,
        move |entry, length| {
            let (title, description, message) = match entry {
                NewChatEntry::Direct => (
                    "Direct message",
                    "One person, by address",
                    Message::OpenNewDm,
                ),
                NewChatEntry::Group => {
                    ("Group", "Named, with members", Message::OpenNewGroup)
                }
            };

            button(
                column![
                    text(title).style(theme::text::primary).font_maybe(
                        theme::font_style::primary(theme).map(font::get)
                    ),
                    text(description)
                        .size(theme::TEXT_SIZE - 2.0)
                        .style(theme::text::secondary)
                        .font_maybe(
                            theme::font_style::secondary(theme).map(font::get)
                        ),
                ]
                .spacing(1),
            )
            .width(length)
            .padding(config.context_menu.padding.entry)
            .on_press(message)
            .into()
        },
    )
    .into()
}

#[derive(Debug, Clone, Copy)]
enum NewChatEntry {
    Direct,
    Group,
}

fn unread_pill<'a>(count: usize) -> Element<'a, Message> {
    let label = if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };

    container(
        text(label)
            .size(theme::TEXT_SIZE - 4.0)
            .line_height(LineHeight::Relative(1.0))
            .style(theme::text::unread_indicator),
    )
    .style(|theme: &Theme| container::Style {
        background: Some(theme.styles().buttons.primary.background.into()),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .padding([2, 5])
    .into()
}

fn buffer_press_message(
    config: &Config,
    buffer: data::Buffer,
    open_as_window_pane: Option<(window::Id, pane_grid::Pane)>,
    focused_as_window_pane: Option<(window::Id, pane_grid::Pane)>,
) -> Message {
    match focused_as_window_pane {
        Some((window, pane)) => {
            if let Some(focus_action) = config.actions.sidebar.focused_buffer {
                match focus_action {
                    BufferFocusedAction::ClosePane => {
                        Message::Close(window, pane)
                    }
                }
            } else {
                // Re-focus pane on press instead of disabling the button in
                // order to have hover status of the button for styling
                Message::Focus(window, pane)
            }
        }
        None => {
            if let Some((window, pane)) = open_as_window_pane {
                Message::Focus(window, pane)
            } else {
                match config.actions.sidebar.buffer {
                    BufferAction::NewPane => Message::New(buffer),
                    BufferAction::ReplacePane => Message::Replace(buffer),
                    BufferAction::NewWindow => Message::Popout(buffer),
                }
            }
        }
    }
}

fn conversation_button<'a>(
    config: &'a Config,
    panes: &'a Panes,
    focus: Focus,
    conversation: &'a data::Conversation,
    history: &'a history::Manager,
    width: Length,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let kind = history::Kind::Conversation(conversation.id.clone());

    let open_as_window_pane =
        panes.iter().find_map(|(window_id, pane, state)| {
            (state.buffer.convo_id() == Some(&conversation.id))
                .then_some((window_id, pane))
        });

    let focused_as_window_pane =
        panes.iter().find_map(|(window_id, pane, state)| {
            (Focus {
                window: window_id,
                pane,
            } == focus
                && state.buffer.convo_id() == Some(&conversation.id))
            .then_some((window_id, pane))
        });

    let is_visible_pane = open_as_window_pane.is_some();

    let unread_count = history.unread_count(&kind);
    let show_unread = unread_count > 0
        && (config.sidebar.unread_indicator.show_on_open_buffers
            || !is_visible_pane);

    let title_style = if show_unread && config.sidebar.unread_indicator.title {
        theme::text::unread_indicator
    } else {
        theme::text::primary
    };

    // The unread row leans on weight (QML parity) on top of the
    // configurable unread color.
    let title = text(conversation.display_name())
        .line_height(LineHeight::Relative(1.0))
        .size_maybe(
            config
                .sidebar
                .primary_font_size
                .or(config.sidebar.secondary_font_size)
                .or(config.font.size)
                .map(f32::from),
        )
        .style(title_style)
        .font(font::get(if show_unread {
            FontStyle::Bold
        } else {
            theme::font_style::primary(theme).unwrap_or_default()
        }))
        .shaping(Shaping::Advanced)
        .wrapping(Wrapping::None)
        .ellipsis(Ellipsis::End);

    let preview_text = conversation.preview.as_deref().map(|preview| {
        preview
            .char_indices()
            .nth(PREVIEW_DISPLAY_CHARS)
            .map_or_else(
                || preview.to_string(),
                |(index, _)| format!("{}…", &preview[..index]),
            )
    });

    let preview =
        text(preview_text.unwrap_or_else(|| "No messages yet".to_string()))
            .size(
                config
                    .sidebar
                    .secondary_font_size
                    .or(config.font.size)
                    .map_or(theme::TEXT_SIZE - 1.0, f32::from),
            )
            .style(if show_unread && conversation.preview.is_some() {
                theme::text::secondary
            } else {
                theme::text::tertiary
            })
            .font_maybe(
                conversation
                    .preview
                    .is_none()
                    .then(|| font::get(FontStyle::Italic)),
            )
            .shaping(Shaping::Advanced)
            .wrapping(Wrapping::None)
            .ellipsis(Ellipsis::End);

    let time_label =
        text(data::time::relative_day_label(conversation.last_activity))
            .size(theme::TEXT_SIZE - 2.0)
            .style(if show_unread {
                theme::text::unread_indicator
            } else {
                theme::text::tertiary
            });

    let pill = (show_unread && config.sidebar.unread_indicator.has_icon())
        .then(|| unread_pill(unread_count));

    let details = column![title, preview].spacing(1).width(width);

    let meta = column![time_label, pill].spacing(2).align_x(Alignment::End);

    let content = row![
        avatar::conversation(conversation, AVATAR_SIZE),
        details,
        meta
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);

    let base =
        button(content.width(width).padding(Padding::default().bottom(1)))
            .style(move |theme, status| {
                theme::button::sidebar_buffer(
                    theme,
                    status,
                    focused_as_window_pane.is_some(),
                    open_as_window_pane.is_some(),
                )
            })
            .padding(config.sidebar.padding.buffer)
            .on_press(buffer_press_message(
                config,
                data::Buffer::Conversation(conversation.id.clone()),
                open_as_window_pane,
                focused_as_window_pane,
            ));

    let can_mark_as_read = history.can_mark_as_read(&kind);

    let entries = Entry::list(
        panes.len(),
        open_as_window_pane,
        focus,
        true,
        Some(conversation.kind),
    );

    context_menu(
        context_menu::MouseButton::default(),
        context_menu::Anchor::Cursor,
        context_menu::ToggleBehavior::KeepOpen,
        Some(mouse::Interaction::Pointer),
        base,
        entries,
        move |entry, length| {
            entry_button(
                entry,
                conversation.display_name(),
                data::Buffer::Conversation(conversation.id.clone()),
                can_mark_as_read,
                Some(conversation),
                length,
                config,
                theme,
            )
        },
    )
    .into()
}

fn internal_buffer_button<'a>(
    config: &'a Config,
    panes: &'a Panes,
    focus: Focus,
    buffer: buffer::Internal,
    history: &'a history::Manager,
    width: Length,
    theme: &'a Theme,
) -> Option<Element<'a, Message>> {
    let kind = history::Kind::from_buffer(data::Buffer::Internal(buffer));

    let open_as_window_pane =
        panes.iter().find_map(|(window_id, pane, state)| {
            (state.buffer.internal() == Some(buffer))
                .then_some((window_id, pane))
        });

    let focused_as_window_pane =
        panes.iter().find_map(|(window_id, pane, state)| {
            (Focus {
                window: window_id,
                pane,
            } == focus
                && state.buffer.internal() == Some(buffer))
            .then_some((window_id, pane))
        });

    let has_unread = kind.as_ref().is_some_and(|kind| history.has_unread(kind));

    let muted = config
        .sidebar
        .internal_buffers
        .mute
        .iter()
        .any(|muted| buffer::Internal::from(muted) == buffer);

    if muted && !has_unread && open_as_window_pane.is_none() {
        return None;
    }

    let icon = match buffer {
        buffer::Internal::ConfigEditor => icon::config(),
        buffer::Internal::Logs => {
            if has_unread {
                icon::logs().style(theme::text::tertiary)
            } else {
                icon::logs()
            }
        }
    };

    let title: &'static str = (&buffer).into();

    let content = row![
        container(icon).width(12.0).height(12.0),
        text(title)
            .line_height(LineHeight::Relative(1.0))
            .size_maybe(
                config
                    .sidebar
                    .primary_font_size
                    .or(config.sidebar.secondary_font_size)
                    .or(config.font.size)
                    .map(f32::from),
            )
            .style(if has_unread {
                theme::text::tertiary
            } else {
                theme::text::primary
            })
            .font_maybe(theme::font_style::primary(theme).map(font::get))
            .shaping(Shaping::Advanced)
            .wrapping(Wrapping::None)
            .ellipsis(Ellipsis::End),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);

    let base =
        button(content.width(width).padding(Padding::default().bottom(1)))
            .style(move |theme, status| {
                theme::button::sidebar_buffer(
                    theme,
                    status,
                    focused_as_window_pane.is_some(),
                    open_as_window_pane.is_some(),
                )
            })
            .padding(config.sidebar.padding.buffer)
            .on_press(buffer_press_message(
                config,
                data::Buffer::Internal(buffer),
                open_as_window_pane,
                focused_as_window_pane,
            ));

    let can_mark_as_read = kind
        .as_ref()
        .is_some_and(|kind| history.can_mark_as_read(kind));

    let entries = Entry::list(
        panes.len(),
        open_as_window_pane,
        focus,
        kind.is_some(),
        None,
    );

    Some(
        context_menu(
            context_menu::MouseButton::default(),
            context_menu::Anchor::Cursor,
            context_menu::ToggleBehavior::KeepOpen,
            Some(mouse::Interaction::Pointer),
            base,
            entries,
            move |entry, length| {
                entry_button(
                    entry,
                    title.to_string(),
                    data::Buffer::Internal(buffer),
                    can_mark_as_read,
                    None,
                    length,
                    config,
                    theme,
                )
            },
        )
        .into(),
    )
}

fn entry_button<'a>(
    entry: Entry,
    title: String,
    buffer: data::Buffer,
    can_mark_as_read: bool,
    conversation: Option<&data::Conversation>,
    length: Length,
    config: &'a Config,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let (content, message) = match entry {
        Entry::Context => {
            return container(
                text(title)
                    .style(theme::text::primary)
                    .font_maybe(
                        theme::font_style::primary(theme).map(font::get),
                    )
                    .width(length),
            )
            .padding(config.context_menu.padding.entry)
            .into();
        }
        Entry::HorizontalRule => match length {
            Length::Fill => {
                return container(rule::horizontal(1)).padding([0, 6]).into();
            }
            _ => {
                return Space::new().width(length).height(1).into();
            }
        },
        Entry::MarkAsRead => (
            "Mark as read",
            can_mark_as_read.then(|| Message::MarkAsRead(buffer)),
        ),
        Entry::NewPane => ("Open in new pane", Some(Message::New(buffer))),
        Entry::Popout => ("Open in new window", Some(Message::Popout(buffer))),
        Entry::Replace => {
            ("Replace current pane", Some(Message::Replace(buffer)))
        }
        Entry::Close(window, pane) => {
            ("Close pane", Some(Message::Close(window, pane)))
        }
        Entry::Swap(window, pane) => {
            ("Swap with current pane", Some(Message::Swap(window, pane)))
        }
        Entry::CopyConvoId => (
            "Copy conversation ID",
            conversation.map(|conversation| {
                Message::CopyText(conversation.id.to_string())
            }),
        ),
        // Disabled until the peer is known (members load lazily).
        Entry::CopyAddress => (
            "Copy address",
            conversation
                .and_then(data::Conversation::peer_address)
                .map(|address| Message::CopyText(address.to_string())),
        ),
        Entry::SetNickname => (
            "Set nickname",
            conversation.map(|conversation| {
                Message::OpenSetNickname(conversation.id.clone())
            }),
        ),
        Entry::Delete => (
            "Delete conversation",
            conversation.map(|conversation| {
                Message::ConfirmDelete(conversation.id.clone())
            }),
        ),
    };

    button(text(content))
        .width(length)
        .padding(config.context_menu.padding.entry)
        .style(|theme, status| theme::button::primary(theme, status, false))
        .on_press_maybe(message)
        .into()
}
