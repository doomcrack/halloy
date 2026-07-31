use data::config;
use data::conversation::ConvoId;
use iced::widget::{button, container, text};
use iced::{Length, Task, alignment};

use crate::widget::Element;
use crate::{Theme, open_url, theme, window};

pub mod about;
pub mod add_member;
pub mod confirm_delete;
pub mod member_add_info;
pub mod new_dm;
pub mod new_group;
pub mod prompt_before_open_url;
pub mod reload_configuration_error;
pub mod set_nickname;

#[derive(Debug)]
pub enum Modal {
    ReloadConfigurationError(config::Error),
    About(about::About),
    PromptBeforeOpenUrl { url: String, window: window::Id },
    NewDm(new_dm::NewDm),
    NewGroup(new_group::NewGroup),
    SetNickname(set_nickname::SetNickname),
    ConfirmDeleteConversation(confirm_delete::ConfirmDelete),
    AddMember(add_member::AddMember),
    MemberAddInfo(member_add_info::MemberAddInfo),
}

#[derive(Debug, Clone)]
pub enum Message {
    Cancel,
    OpenURL(String),
    // Modal specific messages
    About(about::Action),
    NewDm(new_dm::Message),
    NewGroup(new_group::Message),
    SetNickname(set_nickname::Message),
    ConfirmDelete(confirm_delete::Message),
    AddMember(add_member::Message),
    MemberAddInfo(member_add_info::Message),
}

pub enum Event {
    CloseModal,
    CreateDirectMessage {
        peer_address: String,
    },
    CreateGroup {
        name: String,
        description: String,
    },
    SetNickname {
        convo_id: ConvoId,
        nickname: String,
    },
    DeleteConversation(ConvoId),
    /// An address was entered in the add-member dialog; the caller runs
    /// the first-add explainer gate before sending.
    AddGroupMember {
        convo_id: ConvoId,
        peer_address: String,
    },
    /// The explainer was confirmed; send the invite (and persist the
    /// don't-show-again preference when asked).
    ConfirmAddGroupMember {
        convo_id: ConvoId,
        peer_address: String,
        dont_show_again: bool,
    },
}

/// Dialog drafts that outlive a dismissed modal (QML parity: a dismissed
/// dialog reopens with the same draft; it clears on a successful create).
#[derive(Debug, Clone, Default)]
pub struct Drafts {
    pub dm: String,
    pub group_name: String,
    pub group_description: String,
    pub member: String,
}

impl Modal {
    pub fn window_id(&self) -> Option<window::Id> {
        match self {
            Modal::ReloadConfigurationError(..)
            | Modal::About(..)
            | Modal::NewDm(..)
            | Modal::NewGroup(..)
            | Modal::SetNickname(..)
            | Modal::ConfirmDeleteConversation(..)
            | Modal::AddMember(..)
            | Modal::MemberAddInfo(..) => None,
            Modal::PromptBeforeOpenUrl { url: _, window } => Some(*window),
        }
    }

    /// Focuses the modal's primary input, if it has one.
    pub fn focus(&self) -> Task<Message> {
        match self {
            Modal::NewDm(modal) => modal.focus(),
            Modal::NewGroup(modal) => modal.focus(),
            Modal::SetNickname(modal) => modal.focus(),
            Modal::AddMember(modal) => modal.focus(),
            Modal::ReloadConfigurationError(..)
            | Modal::About(..)
            | Modal::PromptBeforeOpenUrl { .. }
            | Modal::ConfirmDeleteConversation(..)
            | Modal::MemberAddInfo(..) => Task::none(),
        }
    }

    pub fn update(
        &mut self,
        message: Message,
    ) -> (Task<Message>, Option<Event>) {
        match message {
            Message::Cancel => (Task::none(), Some(Event::CloseModal)),
            Message::About(action) => {
                if let Modal::About(about) = self {
                    (about.update(action), None)
                } else {
                    (Task::none(), None)
                }
            }
            Message::NewDm(message) => {
                if let Modal::NewDm(modal) = self {
                    modal.update(message)
                } else {
                    (Task::none(), None)
                }
            }
            Message::NewGroup(message) => {
                if let Modal::NewGroup(modal) = self {
                    modal.update(message)
                } else {
                    (Task::none(), None)
                }
            }
            Message::SetNickname(message) => {
                if let Modal::SetNickname(modal) = self {
                    modal.update(message)
                } else {
                    (Task::none(), None)
                }
            }
            Message::ConfirmDelete(message) => {
                if let Modal::ConfirmDeleteConversation(modal) = self {
                    modal.update(message)
                } else {
                    (Task::none(), None)
                }
            }
            Message::AddMember(message) => {
                if let Modal::AddMember(modal) = self {
                    modal.update(message)
                } else {
                    (Task::none(), None)
                }
            }
            Message::MemberAddInfo(message) => {
                if let Modal::MemberAddInfo(modal) = self {
                    modal.update(message)
                } else {
                    (Task::none(), None)
                }
            }
            Message::OpenURL(raw_url) => {
                let canonical = url::Url::parse(&raw_url)
                    .map_or(raw_url, |u| u.to_string());
                let _ = open_url::open(canonical);
                (Task::none(), Some(Event::CloseModal))
            }
        }
    }

    pub fn view<'a>(&'a self, theme: &'a Theme) -> Element<'a, Message> {
        match self {
            Modal::ReloadConfigurationError(error) => {
                reload_configuration_error::view(error, theme)
            }
            Modal::About(about) => about.view(theme),
            Modal::PromptBeforeOpenUrl { url, window: _ } => {
                prompt_before_open_url::view(url, theme)
            }
            Modal::NewDm(modal) => modal.view(theme),
            Modal::NewGroup(modal) => modal.view(theme),
            Modal::SetNickname(modal) => modal.view(theme),
            Modal::ConfirmDeleteConversation(modal) => modal.view(theme),
            Modal::AddMember(modal) => modal.view(theme),
            Modal::MemberAddInfo(modal) => modal.view(theme),
        }
    }
}

/// A dialog's footer button; `None` renders it disabled.
fn action_button(
    label: &str,
    on_press: Option<Message>,
) -> Element<'_, Message> {
    button(
        container(text(label))
            .align_x(alignment::Horizontal::Center)
            .width(Length::Fill),
    )
    .padding(5)
    .width(Length::Fixed(96.0))
    .style(|theme, status| theme::button::secondary(theme, status, false))
    .on_press_maybe(on_press)
    .into()
}
