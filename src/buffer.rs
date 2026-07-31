use data::address::Address;
use data::conversation::ConvoId;
use data::{Config, buffer, history};
use iced::advanced::widget;
use iced::advanced::widget::operation::focusable;
use iced::{Size, Task};

pub use self::config_editor::ConfigEditor;
pub use self::conversation::Conversation;
pub use self::logs::Logs;
use crate::Theme;
use crate::screen::dashboard::sidebar;
use crate::widget::Element;

pub mod config_editor;
pub mod context_menu;
pub mod conversation;
pub mod empty;
mod input_view;
pub mod logs;
mod message_view;
mod scroll_view;

#[derive(Clone, Debug)]
pub enum Buffer {
    Empty,
    Conversation(Conversation),
    Logs(Logs),
    ConfigEditor(ConfigEditor),
}

#[derive(Debug, Clone)]
pub enum Message {
    Conversation(conversation::Message),
    Logs(logs::Message),
    ConfigEditor(config_editor::Message),
}

pub enum Event {
    ContextMenu(context_menu::Event),
    OpenUrl(String),
    MarkAsRead(history::Kind),
    /// Enter-send from the composer; the dashboard forwards this to the
    /// backend as `Control::SendMessage`.
    SendMessage {
        convo_id: ConvoId,
        content: String,
    },
    /// A parsed slash command from the composer.
    Command(data::Command),
    ConfigSaved,
    /// Copy-to-clipboard requests from the roster and details panels.
    CopyText(String),
    /// Open (or create) a direct conversation with the address.
    OpenDm(String),
    /// Open the add-member dialog for this group conversation.
    OpenAddMember(ConvoId),
}

impl Buffer {
    pub fn from_data(
        buffer: data::Buffer,
        history: &history::Manager,
        pane_size: Size,
        config: &Config,
    ) -> Self {
        match buffer {
            data::Buffer::Conversation(convo_id) => Self::Conversation(
                Conversation::new(convo_id, history, pane_size, config),
            ),
            data::Buffer::Internal(internal) => match internal {
                buffer::Internal::Logs => {
                    Self::Logs(Logs::new(pane_size, config))
                }
                buffer::Internal::ConfigEditor => {
                    Self::ConfigEditor(ConfigEditor::new())
                }
            },
        }
    }

    pub fn empty() -> Self {
        Self::Empty
    }

    pub fn convo_id(&self) -> Option<&ConvoId> {
        match self {
            Buffer::Conversation(state) => Some(&state.convo_id),
            Buffer::Empty | Buffer::Logs(_) | Buffer::ConfigEditor(_) => None,
        }
    }

    pub fn internal(&self) -> Option<buffer::Internal> {
        match self {
            Buffer::Empty | Buffer::Conversation(_) => None,
            Buffer::Logs(_) => Some(buffer::Internal::Logs),
            Buffer::ConfigEditor(_) => Some(buffer::Internal::ConfigEditor),
        }
    }

    pub fn data(&self) -> Option<data::Buffer> {
        match self {
            Buffer::Empty => None,
            Buffer::Conversation(state) => {
                Some(data::Buffer::Conversation(state.convo_id.clone()))
            }
            Buffer::Logs(_) => {
                Some(data::Buffer::Internal(buffer::Internal::Logs))
            }
            Buffer::ConfigEditor(_) => {
                Some(data::Buffer::Internal(buffer::Internal::ConfigEditor))
            }
        }
    }

    pub fn update(
        &mut self,
        message: Message,
        history: &mut history::Manager,
        config: &Config,
    ) -> (Task<Message>, Option<Event>) {
        match (self, message) {
            (Buffer::Conversation(state), Message::Conversation(message)) => {
                let (command, event) = state.update(message, history, config);

                let event = event.map(|event| match event {
                    conversation::Event::ContextMenu(event) => {
                        Event::ContextMenu(event)
                    }
                    conversation::Event::MarkAsRead(kind) => {
                        Event::MarkAsRead(kind)
                    }
                    conversation::Event::OpenUrl(url) => Event::OpenUrl(url),
                    conversation::Event::SendMessage { convo_id, content } => {
                        Event::SendMessage { convo_id, content }
                    }
                    conversation::Event::Command(command) => {
                        Event::Command(command)
                    }
                    conversation::Event::CopyText(text) => {
                        Event::CopyText(text)
                    }
                    conversation::Event::OpenDm(address) => {
                        Event::OpenDm(address)
                    }
                    conversation::Event::OpenAddMember(convo_id) => {
                        Event::OpenAddMember(convo_id)
                    }
                });

                (command.map(Message::Conversation), event)
            }
            (Buffer::Logs(state), Message::Logs(message)) => {
                let (command, event) = state.update(message, history, config);

                let event = event.map(|event| match event {
                    logs::Event::ContextMenu(event) => {
                        Event::ContextMenu(event)
                    }
                    logs::Event::MarkAsRead => {
                        Event::MarkAsRead(history::Kind::Logs)
                    }
                    logs::Event::OpenUrl(url) => Event::OpenUrl(url),
                });

                (command.map(Message::Logs), event)
            }
            (Buffer::ConfigEditor(state), Message::ConfigEditor(message)) => {
                let (command, event) = state.update(message, config);

                let event = event.map(|event| match event {
                    config_editor::Event::ConfigSaved => Event::ConfigSaved,
                });

                (command.map(Message::ConfigEditor), event)
            }
            _ => (Task::none(), None),
        }
    }

    pub fn view<'a>(
        &'a self,
        session: &'a data::Session,
        history: &'a history::Manager,
        settings: Option<&'a buffer::Settings>,
        config: &'a Config,
        theme: &'a Theme,
        is_focused: bool,
        sidebar: &'a sidebar::Sidebar,
    ) -> Element<'a, Message> {
        match self {
            Buffer::Empty => empty::view(config, sidebar),
            Buffer::Conversation(state) => conversation::view(
                state,
                session.conversations.get(&state.convo_id),
                our_address(session),
                session.delivery.can_act(),
                history,
                settings,
                config,
                theme,
                is_focused,
            )
            .map(Message::Conversation),
            Buffer::Logs(state) => {
                logs::view(state, history, config, theme).map(Message::Logs)
            }
            Buffer::ConfigEditor(state) => {
                config_editor::view(state, config, theme)
                    .map(Message::ConfigEditor)
            }
        }
    }

    pub fn focus(&self) -> Task<Message> {
        match self {
            Buffer::Empty | Buffer::Logs(_) => {
                widget::operate(focusable::unfocus())
            }
            Buffer::ConfigEditor(config_editor) => {
                config_editor.focus().map(Message::ConfigEditor)
            }
            Buffer::Conversation(conversation) => {
                conversation.focus().map(Message::Conversation)
            }
        }
    }

    pub fn reset(&mut self) {
        match self {
            Buffer::Empty | Buffer::Logs(_) | Buffer::ConfigEditor(_) => {}
            Buffer::Conversation(conversation) => conversation.reset(),
        }
    }

    /// Tells the thread showing `kind` that its messages landed, so its
    /// settle window starts from there.
    pub fn messages_loaded(&mut self, kind: &history::Kind) -> Task<Message> {
        if let Buffer::Conversation(conversation) = self
            && let history::Kind::Conversation(convo_id) = kind
            && conversation.convo_id == *convo_id
        {
            conversation.messages_loaded().map(Message::Conversation)
        } else {
            Task::none()
        }
    }

    /// Restores an unsent draft into the composer after a failed send.
    pub fn restore_draft(&mut self, convo_id: &ConvoId, content: &str) {
        if let Buffer::Conversation(conversation) = self
            && conversation.convo_id == *convo_id
        {
            conversation.restore_draft(content);
        }
    }

    pub fn scroll_up_page(&mut self) -> Task<Message> {
        match self {
            Buffer::Empty => Task::none(),
            Buffer::ConfigEditor(state) => {
                state.scroll_up_page();
                Task::none()
            }
            Buffer::Conversation(conversation) => {
                conversation.scroll_view.scroll_up_page().map(|message| {
                    Message::Conversation(conversation::Message::ScrollView(
                        message,
                    ))
                })
            }
            Buffer::Logs(log) => {
                log.scroll_view.scroll_up_page().map(|message| {
                    Message::Logs(logs::Message::ScrollView(message))
                })
            }
        }
    }

    pub fn scroll_down_page(&mut self) -> Task<Message> {
        match self {
            Buffer::Empty => Task::none(),
            Buffer::ConfigEditor(state) => {
                state.scroll_down_page();
                Task::none()
            }
            Buffer::Conversation(conversation) => {
                conversation.scroll_view.scroll_down_page().map(|message| {
                    Message::Conversation(conversation::Message::ScrollView(
                        message,
                    ))
                })
            }
            Buffer::Logs(log) => {
                log.scroll_view.scroll_down_page().map(|message| {
                    Message::Logs(logs::Message::ScrollView(message))
                })
            }
        }
    }

    pub fn scroll_to_start(&mut self, config: &Config) -> Task<Message> {
        match self {
            Buffer::Empty => Task::none(),
            Buffer::ConfigEditor(state) => {
                state.scroll_to_start();
                Task::none()
            }
            Buffer::Conversation(conversation) => conversation
                .scroll_view
                .scroll_to_start(config)
                .map(|message| {
                    Message::Conversation(conversation::Message::ScrollView(
                        message,
                    ))
                }),
            Buffer::Logs(log) => {
                log.scroll_view.scroll_to_start(config).map(|message| {
                    Message::Logs(logs::Message::ScrollView(message))
                })
            }
        }
    }

    pub fn scroll_to_end(&mut self, config: &Config) -> Task<Message> {
        match self {
            Buffer::Empty => Task::none(),
            Buffer::ConfigEditor(state) => {
                state.scroll_to_end();
                Task::none()
            }
            Buffer::Conversation(conversation) => conversation
                .scroll_view
                .scroll_to_end(config)
                .map(|message| {
                    Message::Conversation(conversation::Message::ScrollView(
                        message,
                    ))
                }),
            Buffer::Logs(log) => {
                log.scroll_view.scroll_to_end(config).map(|message| {
                    Message::Logs(logs::Message::ScrollView(message))
                })
            }
        }
    }

    pub fn scroll_to_backlog(
        &mut self,
        history: &history::Manager,
        config: &Config,
    ) -> Task<Message> {
        match self {
            Buffer::Empty | Buffer::ConfigEditor(_) => Task::none(),
            Buffer::Conversation(state) => {
                let kind = scroll_view::Kind::Conversation(&state.convo_id);

                state
                    .scroll_view
                    .scroll_to_backlog(kind, history, config)
                    .map(|message| {
                        Message::Conversation(
                            conversation::Message::ScrollView(message),
                        )
                    })
            }
            Buffer::Logs(state) => state
                .scroll_view
                .scroll_to_backlog(scroll_view::Kind::Logs, history, config)
                .map(|message| {
                    Message::Logs(logs::Message::ScrollView(message))
                }),
        }
    }

    pub fn has_pending_scroll_to(&self) -> bool {
        match self {
            Buffer::Empty | Buffer::ConfigEditor(_) => false,
            Buffer::Conversation(state) => {
                state.scroll_view.has_pending_scroll_to()
            }
            Buffer::Logs(state) => state.scroll_view.has_pending_scroll_to(),
        }
    }

    pub fn prepare_for_pending_scroll_to(
        &mut self,
        history: &history::Manager,
        config: &Config,
    ) -> Task<Message> {
        match self {
            Buffer::Empty | Buffer::ConfigEditor(_) => Task::none(),
            Buffer::Conversation(state) => {
                let kind = scroll_view::Kind::Conversation(&state.convo_id);

                state
                    .scroll_view
                    .prepare_for_pending_scroll_to(kind, history, config)
                    .map(|message| {
                        Message::Conversation(
                            conversation::Message::ScrollView(message),
                        )
                    })
            }
            Buffer::Logs(state) => state
                .scroll_view
                .prepare_for_pending_scroll_to(
                    scroll_view::Kind::Logs,
                    history,
                    config,
                )
                .map(|message| {
                    Message::Logs(logs::Message::ScrollView(message))
                }),
        }
    }

    pub fn is_scrolled_to_bottom(&self) -> Option<bool> {
        match self {
            Buffer::Empty | Buffer::ConfigEditor(_) => None,
            Buffer::Conversation(conversation) => {
                Some(conversation.scroll_view.is_scrolled_to_bottom())
            }
            Buffer::Logs(log) => Some(log.scroll_view.is_scrolled_to_bottom()),
        }
    }

    pub fn close_picker(&mut self) -> bool {
        match self {
            Buffer::Empty | Buffer::Logs(_) | Buffer::ConfigEditor(_) => false,
            Buffer::Conversation(state) => state.input_view.close_picker(),
        }
    }

    pub fn update_pane_size(&mut self, pane_size: Size, config: &Config) {
        match self {
            Buffer::Empty | Buffer::ConfigEditor(_) => (),
            Buffer::Conversation(conversation) => {
                conversation.scroll_view.update_pane_size(pane_size, config);
            }
            Buffer::Logs(log) => {
                log.scroll_view.update_pane_size(pane_size, config);
            }
        }
    }
}

/// The identity address for sender-run comparison and own-message styling.
fn our_address(session: &data::Session) -> Option<&Address> {
    session.identity.as_ref().map(|identity| &identity.address)
}
