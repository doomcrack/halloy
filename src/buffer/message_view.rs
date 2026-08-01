use data::Config;
use data::address::Address;
use data::buffer::{Alignment, RightAlignmentWidths};
use data::message::{self, Source};
use iced::padding;
use iced::widget::{Space, column, container, row, text};

use crate::buffer::scroll_view::{LayoutMessage, Message, sender_label};
use crate::widget::{
    Element, Marker, message_content, message_marker, selectable_text,
};
use crate::{Theme, font, theme};

/// The one message layout: direct and group conversations resolve at view
/// time. Sender identity comes from the message [`Source`] (Peer vs
/// Yourself); sender colors are seeded by the short label so they agree
/// with the avatar ramp (QML parity).
#[derive(Clone, Copy)]
pub struct ConversationLayout<'a> {
    pub config: &'a Config,
    pub theme: &'a Theme,
    pub our_address: Option<&'a Address>,
    /// Only a group names its senders; a direct thread's header already
    /// names the one other participant (QML `groupContext`).
    pub is_group: bool,
}

impl<'a> ConversationLayout<'a> {
    fn format_timestamp(
        &self,
        message: &'a data::Message,
        hide_timestamp: bool,
    ) -> Option<Element<'a, Message>> {
        self.config
            .buffer
            .format_timestamp(&message.server_time)
            .map(|timestamp| {
                if hide_timestamp {
                    let width =
                        font::width_from_str(&timestamp, &self.config.font);

                    return Space::new().width(width).into();
                }

                selectable_text(timestamp)
                    .style(theme::selectable_text::timestamp)
                    .font_maybe(
                        theme::font_style::timestamp(self.theme).map(font::get),
                    )
                    .into()
            })
    }

    /// The sender label, where the thread shows one: a group names its
    /// sender at the head of each run of theirs, a direct thread names
    /// nobody. A hidden label still holds its column open, so a run stays
    /// aligned under the name that heads it.
    fn sender_element(
        &self,
        message: &'a data::Message,
        seed: &'a str,
        own: bool,
        right_alignment_middle_width: Option<f32>,
        hide_sender: bool,
    ) -> Option<Element<'a, Message>> {
        let label = sender_label(message).unwrap_or(seed);

        if hide_sender || !self.is_group {
            let width = right_alignment_middle_width.or_else(|| {
                self.is_group.then(|| {
                    font::width_from_str(label, &self.config.font) + 1.0
                })
            })?;

            return Some(Space::new().width(width).into());
        }

        let config = self.config;
        let label =
            selectable_text(config.buffer.sender.brackets.format(label))
                .style(move |theme| {
                    if own {
                        theme::selectable_text::own_sender(theme)
                    } else {
                        theme::selectable_text::sender(
                            theme,
                            config,
                            Some(seed),
                        )
                    }
                })
                .font_maybe(
                    theme::font_style::sender(self.theme).map(font::get),
                );

        Some(if let Some(width) = right_alignment_middle_width {
            container(label)
                .width(width)
                .align_x(text::Alignment::Right)
                .into()
        } else {
            label.into()
        })
    }

    fn content_on_new_line(&self, message: &data::Message) -> bool {
        matches!(
            (message.target.source(), self.config.buffer.sender.alignment,),
            (Source::Peer(_) | Source::Yourself, Alignment::Top)
        )
    }
}

impl<'a> LayoutMessage<'a> for ConversationLayout<'a> {
    fn format(
        &self,
        message: &'a data::Message,
        right_alignment_widths: Option<RightAlignmentWidths>,
        hide_timestamp: bool,
        hide_sender: bool,
    ) -> Option<Element<'a, Message>> {
        let mut timestamp: Option<Element<_>> =
            self.format_timestamp(message, hide_timestamp);

        if let Some(right_alignment_widths) = right_alignment_widths {
            timestamp = Some(timestamp.map_or(
                Space::new().width(right_alignment_widths.timestamp).into(),
                |timestamp| {
                    container(timestamp)
                        .width(right_alignment_widths.timestamp)
                        .into()
                },
            ));
        }

        let right_alignment_middle_width = right_alignment_widths
            .map(|right_alignment_widths| right_alignment_widths.middle);

        let (middle, content): (
            Option<Element<'a, Message>>,
            Element<'a, Message>,
        ) = match message.target.source() {
            source @ (Source::Peer(_) | Source::Yourself) => {
                let (seed, own) = match source {
                    Source::Peer(address) => (address.short_label(), false),
                    _ => (
                        self.our_address.map_or("you", Address::short_label),
                        true,
                    ),
                };

                let sender = self.sender_element(
                    message,
                    seed,
                    own,
                    right_alignment_middle_width,
                    hide_sender,
                );

                let content = message_content(
                    &message.content,
                    self.theme,
                    Message::Link,
                    theme::selectable_text::default,
                    theme::font_style::primary,
                    self.config,
                );

                (sender, content)
            }
            Source::Status(kind) => {
                let kind = *kind;
                let message_style = move |message_theme: &Theme| {
                    theme::selectable_text::status(message_theme, kind)
                };

                let marker = message_marker(
                    Marker::Dot,
                    right_alignment_middle_width,
                    self.config,
                    message_style,
                    None,
                );

                let content = message_content(
                    &message.content,
                    self.theme,
                    Message::Link,
                    message_style,
                    move |message_theme| {
                        theme::font_style::status(message_theme, kind)
                    },
                    self.config,
                );

                (Some(marker), content)
            }
            // Log records render through the log buffers' own layout.
            Source::Internal(
                message::source::Internal::Logs(_)
                | message::source::Internal::Module(_),
            ) => {
                return None;
            }
        };

        let middle_is_some = middle.is_some();

        let left = row![
            timestamp,
            if hide_timestamp {
                let width = font::width_from_str(" ", &self.config.font);

                Element::from(Space::new().width(width))
            } else {
                Element::from(selectable_text(" "))
            },
            middle,
            middle_is_some.then_some(selectable_text(" ")),
        ];

        let message_element = if self.content_on_new_line(message) {
            container(
                column![left, container(content).padding(padding::left(4))]
                    .spacing(2),
            )
            .into()
        } else {
            container(row![left, content]).into()
        };

        Some(message_element)
    }

    fn collapses_sender_runs(&self) -> bool {
        true
    }
}
