//! Set-nickname dialog: a single field for a conversation's local
//! nickname. Submitting an empty field clears the nickname.

use data::conversation::ConvoId;
use iced::widget::{Space, column, container, row, text, text_input};
use iced::{Length, Task};

use super::{Event, Message as ModalMessage, action_button};
use crate::widget::Element;
use crate::{Theme, font, theme};

#[derive(Debug)]
pub struct SetNickname {
    input_id: iced::advanced::widget::Id,
    convo_id: ConvoId,
    display_name: String,
    nickname: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    NicknameChanged(String),
    Submit,
}

impl SetNickname {
    pub fn new(
        convo_id: ConvoId,
        display_name: String,
        nickname: Option<String>,
    ) -> Self {
        Self {
            input_id: iced::advanced::widget::Id::unique(),
            convo_id,
            display_name,
            nickname: nickname.unwrap_or_default(),
        }
    }

    pub fn focus(&self) -> Task<ModalMessage> {
        iced::advanced::widget::operate(
            iced::advanced::widget::operation::focusable::focus(
                self.input_id.clone(),
            ),
        )
    }

    pub fn update(
        &mut self,
        message: Message,
    ) -> (Task<ModalMessage>, Option<Event>) {
        match message {
            Message::NicknameChanged(nickname) => {
                self.nickname = nickname;
                (Task::none(), None)
            }
            Message::Submit => (
                Task::none(),
                Some(Event::SetNickname {
                    convo_id: self.convo_id.clone(),
                    nickname: self.nickname.trim().to_string(),
                }),
            ),
        }
    }

    pub fn view<'a>(&'a self, theme: &'a Theme) -> Element<'a, ModalMessage> {
        let field = text_input(&self.display_name, &self.nickname)
            .id(self.input_id.clone())
            .padding(8)
            .style(theme::text_input::primary)
            .on_input(|nickname| {
                ModalMessage::SetNickname(Message::NicknameChanged(nickname))
            })
            .on_submit(ModalMessage::SetNickname(Message::Submit));

        container(
            column![
                text("Set nickname").style(theme::text::primary).font_maybe(
                    theme::font_style::primary(theme).map(font::get)
                ),
                text(format!(
                    "Local nickname for \"{}\". Leave empty to clear.",
                    self.display_name
                ))
                .style(theme::text::secondary)
                .font_maybe(theme::font_style::secondary(theme).map(font::get)),
                field,
                row![
                    action_button("Cancel", Some(ModalMessage::Cancel)),
                    Space::new().width(Length::Fill),
                    action_button(
                        "Save",
                        Some(ModalMessage::SetNickname(Message::Submit)),
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
