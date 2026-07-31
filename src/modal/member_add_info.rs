//! First-add explainer: membership commits are asynchronous, so a newly
//! invited member can take up to a minute to appear (QML
//! `MemberAddInfoDialog` parity). Carries the pending invite; confirming
//! sends it, and "Don't show this again" persists via the dashboard.

use data::conversation::ConvoId;
use iced::widget::{Space, checkbox, column, container, row, text};
use iced::{Length, Task};

use super::{Event, Message as ModalMessage, action_button};
use crate::widget::Element;
use crate::{Theme, font, theme};

#[derive(Debug)]
pub struct MemberAddInfo {
    convo_id: ConvoId,
    peer_address: String,
    dont_show_again: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    ToggleDontShowAgain(bool),
    Confirm,
}

impl MemberAddInfo {
    pub fn new(convo_id: ConvoId, peer_address: String) -> Self {
        Self {
            convo_id,
            peer_address,
            dont_show_again: false,
        }
    }

    pub fn update(
        &mut self,
        message: Message,
    ) -> (Task<ModalMessage>, Option<Event>) {
        match message {
            Message::ToggleDontShowAgain(checked) => {
                self.dont_show_again = checked;
                (Task::none(), None)
            }
            Message::Confirm => (
                Task::none(),
                Some(Event::ConfirmAddGroupMember {
                    convo_id: self.convo_id.clone(),
                    peer_address: self.peer_address.clone(),
                    dont_show_again: self.dont_show_again,
                }),
            ),
        }
    }

    pub fn view<'a>(&'a self, theme: &'a Theme) -> Element<'a, ModalMessage> {
        container(
            column![
                text("Adding a member")
                    .style(theme::text::primary)
                    .font_maybe(
                        theme::font_style::primary(theme).map(font::get)
                    ),
                text(
                    "Adding a member isn't instant. Membership updates \
                     apply on a timer, so a new member can take up to a \
                     minute to appear."
                )
                .style(theme::text::secondary)
                .font_maybe(theme::font_style::secondary(theme).map(font::get)),
                checkbox(self.dont_show_again)
                    .label("Don't show this again")
                    .on_toggle(|checked| {
                        ModalMessage::MemberAddInfo(
                            Message::ToggleDontShowAgain(checked),
                        )
                    })
                    .style(theme::checkbox::primary),
                row![
                    action_button("Cancel", Some(ModalMessage::Cancel)),
                    Space::new().width(Length::Fill),
                    action_button(
                        "Add member",
                        Some(ModalMessage::MemberAddInfo(Message::Confirm)),
                    ),
                ],
            ]
            .spacing(12),
        )
        .width(Length::Fixed(420.0))
        .style(theme::container::tooltip)
        .padding(25)
        .into()
    }
}
