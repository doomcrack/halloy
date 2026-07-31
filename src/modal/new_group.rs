//! New Group dialog: collects a name and an optional description (QML
//! `NewGroupDialog` parity). Character counters appear past 75% of each
//! limit and turn error-colored past it; the drafts persist across a
//! dismissal and clear only once the group is created.

use iced::widget::{
    Space, column, container, row, text, text_editor, text_input,
};
use iced::{Length, Task};

use super::{Event, Message as ModalMessage, action_button};
use crate::widget::Element;
use crate::{Theme, font, theme};

/// Character caps for the shared group metadata (QML parity).
const NAME_LIMIT: usize = 64;
const DESCRIPTION_LIMIT: usize = 280;

#[derive(Debug)]
pub struct NewGroup {
    input_id: iced::advanced::widget::Id,
    name: String,
    description: text_editor::Content,
}

#[derive(Debug, Clone)]
pub enum Message {
    NameChanged(String),
    EditDescription(text_editor::Action),
    Submit,
}

impl NewGroup {
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            input_id: iced::advanced::widget::Id::unique(),
            name: name.to_string(),
            description: text_editor::Content::with_text(description),
        }
    }

    pub fn draft(&self) -> (String, String) {
        (self.name.clone(), self.description.text())
    }

    pub fn focus(&self) -> Task<ModalMessage> {
        iced::advanced::widget::operate(
            iced::advanced::widget::operation::focusable::focus(
                self.input_id.clone(),
            ),
        )
    }

    fn is_valid(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.chars().count() <= NAME_LIMIT
            && self.description.text().chars().count() <= DESCRIPTION_LIMIT
    }

    pub fn update(
        &mut self,
        message: Message,
    ) -> (Task<ModalMessage>, Option<Event>) {
        match message {
            Message::NameChanged(name) => {
                self.name = name;
                (Task::none(), None)
            }
            Message::EditDescription(action) => {
                self.description.perform(action);
                (Task::none(), None)
            }
            Message::Submit => {
                if !self.is_valid() {
                    return (Task::none(), None);
                }

                (
                    Task::none(),
                    Some(Event::CreateGroup {
                        name: self.name.trim().to_string(),
                        description: self.description.text().trim().to_string(),
                    }),
                )
            }
        }
    }

    pub fn view<'a>(&'a self, theme: &'a Theme) -> Element<'a, ModalMessage> {
        let name_field = text_input("e.g. Book Club", &self.name)
            .id(self.input_id.clone())
            .padding(8)
            .style(theme::text_input::primary)
            .on_input(|name| ModalMessage::NewGroup(Message::NameChanged(name)))
            .on_submit(ModalMessage::NewGroup(Message::Submit));

        let description_field = text_editor(&self.description)
            .placeholder("What's this group about?")
            .padding(8)
            .height(Length::Fixed(80.0))
            .style(theme::text_editor::primary)
            .on_action(|action| {
                ModalMessage::NewGroup(Message::EditDescription(action))
            });

        container(
            column![
                text("New Group").style(theme::text::primary).font_maybe(
                    theme::font_style::primary(theme).map(font::get)
                ),
                labeled(
                    "Group name",
                    self.name.chars().count(),
                    NAME_LIMIT,
                    theme,
                ),
                name_field,
                labeled(
                    "Description (optional)",
                    self.description.text().chars().count(),
                    DESCRIPTION_LIMIT,
                    theme,
                ),
                description_field,
                text(
                    "The name and description can not be changed after the \
                     group is created."
                )
                .style(theme::text::secondary)
                .font_maybe(theme::font_style::secondary(theme).map(font::get)),
                row![
                    action_button("Cancel", Some(ModalMessage::Cancel)),
                    Space::new().width(Length::Fill),
                    action_button(
                        "Create",
                        self.is_valid()
                            .then_some(ModalMessage::NewGroup(Message::Submit)),
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

/// A field label with its live character counter; the counter appears
/// past 75% of the limit and turns error-colored past it (QML parity).
fn labeled<'a>(
    label: &'a str,
    length: usize,
    limit: usize,
    theme: &'a Theme,
) -> Element<'a, ModalMessage> {
    let counter = (length * 4 > limit * 3).then(|| {
        text(format!("{length}/{limit}"))
            .size(theme::TEXT_SIZE - 1.0)
            .style(if length > limit {
                theme::text::error
            } else {
                theme::text::secondary
            })
            .font_maybe(theme::font_style::secondary(theme).map(font::get))
    });

    row![
        text(label)
            .size(theme::TEXT_SIZE - 1.0)
            .style(theme::text::secondary)
            .font_maybe(theme::font_style::secondary(theme).map(font::get)),
        Space::new().width(Length::Fill),
        counter,
    ]
    .into()
}
