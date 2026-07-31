//! New DM dialog: collects a peer address for a 1:1 conversation (QML
//! `NewConversationDialog` parity). Return submits, the entry is trimmed,
//! and the draft persists across a dismissal — it clears only once a
//! conversation is created.

use iced::widget::{Space, column, container, row, text, text_editor};
use iced::{Length, Task, keyboard};

use super::{Event, Message as ModalMessage, action_button};
use crate::widget::Element;
use crate::{Theme, font, theme};

#[derive(Debug)]
pub struct NewDm {
    input_id: iced::advanced::widget::Id,
    content: text_editor::Content,
}

#[derive(Debug, Clone)]
pub enum Message {
    Edit(text_editor::Action),
    Submit,
}

impl NewDm {
    pub fn new(draft: &str) -> Self {
        Self {
            input_id: iced::advanced::widget::Id::unique(),
            content: text_editor::Content::with_text(draft),
        }
    }

    pub fn draft(&self) -> String {
        self.content.text()
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
            Message::Edit(action) => {
                self.content.perform(action);
                (Task::none(), None)
            }
            Message::Submit => {
                let peer_address = self.content.text().trim().to_string();

                if peer_address.is_empty() {
                    return (Task::none(), None);
                }

                (
                    Task::none(),
                    Some(Event::CreateDirectMessage { peer_address }),
                )
            }
        }
    }

    pub fn view<'a>(&'a self, theme: &'a Theme) -> Element<'a, ModalMessage> {
        let address_field = text_editor(&self.content)
            .id(self.input_id.clone())
            .placeholder("peer address (hex)...")
            .padding(8)
            .height(Length::Fixed(100.0))
            .style(theme::text_editor::primary)
            .on_action(|action| ModalMessage::NewDm(Message::Edit(action)))
            .key_binding(|key_press| {
                if !matches!(
                    key_press.status,
                    text_editor::Status::Focused { .. }
                ) {
                    return None;
                }

                if matches!(
                    key_press.key,
                    keyboard::Key::Named(keyboard::key::Named::Enter)
                ) {
                    Some(text_editor::Binding::Custom(ModalMessage::NewDm(
                        Message::Submit,
                    )))
                } else {
                    text_editor::Binding::from_key_press(key_press)
                }
            });

        let can_submit = !self.content.text().trim().is_empty();

        container(
            column![
                text("New DM").style(theme::text::primary).font_maybe(
                    theme::font_style::primary(theme).map(font::get)
                ),
                text("Paste the other user's address:")
                    .style(theme::text::secondary)
                    .font_maybe(
                        theme::font_style::secondary(theme).map(font::get)
                    ),
                address_field,
                row![
                    action_button("Cancel", Some(ModalMessage::Cancel)),
                    Space::new().width(Length::Fill),
                    action_button(
                        "Create",
                        can_submit
                            .then_some(ModalMessage::NewDm(Message::Submit)),
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
