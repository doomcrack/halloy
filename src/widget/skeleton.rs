//! Loading-state placeholders: dimmed bars in a message-row shape shown
//! while a conversation's history is being fetched from the module (QML
//! `MessageSkeleton` parity).

use iced::widget::{Space, column, container, row};
use iced::{Border, Length};

use super::Element;
use crate::Theme;

/// (sender stub width, text bar fill portion) per placeholder row; four
/// staggered rows, as the QML skeleton stands in with.
const ROWS: &[(u16, u16)] = &[(9, 5), (7, 2), (8, 6), (6, 3)];

/// The placeholder rows, sitting where the real thread's newest messages
/// do: against the bottom of the message area.
pub fn messages<'a, M: 'a>() -> Element<'a, M> {
    column![
        Space::new().height(Length::Fill),
        column(ROWS.iter().map(|(label, text)| bar_row(*label, *text)))
            .spacing(16),
    ]
    .padding(8)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn bar_row<'a, M: 'a>(label_portion: u16, text_portion: u16) -> Element<'a, M> {
    column![
        row![
            bar(Length::FillPortion(label_portion), 10.0),
            Space::new().width(Length::FillPortion(24 - label_portion)),
        ],
        row![
            bar(Length::FillPortion(text_portion), 14.0),
            Space::new().width(Length::FillPortion(8 - text_portion.min(7))),
        ],
    ]
    .spacing(6)
    .into()
}

fn bar<'a, M: 'a>(width: Length, height: f32) -> Element<'a, M> {
    container(Space::new())
        .width(width)
        .height(Length::Fixed(height))
        .style(move |theme: &Theme| container::Style {
            background: Some(theme.styles().general.border.into()),
            border: Border {
                radius: (height / 2.0).into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}
