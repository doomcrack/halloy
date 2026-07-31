//! Delete-conversation confirmation dialog; deletion is irreversible and
//! only proceeds through the explicit Delete action.

use data::conversation::ConvoId;
use iced::widget::{Space, column, container, row, text};
use iced::{Length, Task};

use super::{Event, Message as ModalMessage, action_button};
use crate::widget::Element;
use crate::{Theme, font, theme};

#[derive(Debug)]
pub struct ConfirmDelete {
    convo_id: ConvoId,
    display_name: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Confirm,
}

impl ConfirmDelete {
    pub fn new(convo_id: ConvoId, display_name: String) -> Self {
        Self {
            convo_id,
            display_name,
        }
    }

    pub fn update(
        &mut self,
        message: Message,
    ) -> (Task<ModalMessage>, Option<Event>) {
        match message {
            Message::Confirm => (
                Task::none(),
                Some(Event::DeleteConversation(self.convo_id.clone())),
            ),
        }
    }

    pub fn view<'a>(&'a self, theme: &'a Theme) -> Element<'a, ModalMessage> {
        container(
            column![
                text("Delete conversation")
                    .style(theme::text::primary)
                    .font_maybe(
                        theme::font_style::primary(theme).map(font::get)
                    ),
                text(format!(
                    "Delete \"{}\"? Its messages are removed and this can \
                     not be undone.",
                    self.display_name
                ))
                .style(theme::text::secondary)
                .font_maybe(theme::font_style::secondary(theme).map(font::get)),
                row![
                    action_button("Cancel", Some(ModalMessage::Cancel)),
                    Space::new().width(Length::Fill),
                    delete_button(),
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

fn delete_button<'a>() -> Element<'a, ModalMessage> {
    iced::widget::button(
        container(text("Delete").style(theme::text::error))
            .align_x(iced::alignment::Horizontal::Center)
            .width(Length::Fill),
    )
    .padding(5)
    .width(Length::Fixed(96.0))
    .style(|theme, status| theme::button::secondary(theme, status, false))
    .on_press(ModalMessage::ConfirmDelete(Message::Confirm))
    .into()
}
