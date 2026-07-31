//! Details panel: what the conversation is beyond its name — description,
//! kind, member summary with outstanding invitations named separately,
//! and the copyable identifiers a header has no room for (QML
//! `DetailsPanel` parity). Shown on demand from the pane header or
//! `/details`; costs the layout nothing while closed.

use std::time::Duration;

use data::conversation::{Conversation, Kind};
use iced::widget::{Space, button, column, container, row, text};
use iced::{Length, padding};

use crate::widget::Element;
use crate::{Theme, font, icon, theme};

/// How long a row confirms a copy (QML / account-card parity).
pub const COPY_FLASH_DURATION: Duration = Duration::from_secs(2);

/// The panel's own width (QML `DetailsPanel.implicitWidth`): narrower and a
/// key/value row wraps onto a second line.
pub const WIDTH: f32 = 280.0;

/// Description height cap: roughly five wrapped lines, ellipsized past
/// that (QML clamps the description the same way).
const DESCRIPTION_MAX_HEIGHT: f32 = theme::TEXT_SIZE * 1.3 * 5.0;

/// Above this many chars a description could exceed five wrapped lines
/// at the panel's width, so the height cap kicks in. Shorter text keeps
/// `Shrink` — this fork's `container` has no `max_height`, and a fixed
/// height would otherwise reserve five lines under a one-line note.
const DESCRIPTION_CLAMP_CHARS: usize = 150;

/// The rows whose copy action confirms in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Address,
    ConvoId,
}

#[derive(Debug, Clone)]
pub enum Message {
    Close,
    Copy(Row, String),
    CopyFlashExpired(u64),
}

/// Copy-flash state; generation-counted so a re-copy restarts the timer
/// (the account-card pattern).
#[derive(Debug, Clone, Copy, Default)]
pub struct State {
    generation: u64,
    flashing: Option<Row>,
}

impl State {
    /// Starts a flash on `row`; returns the generation the expiry must
    /// match.
    pub fn copy_started(&mut self, row: Row) -> u64 {
        self.generation += 1;
        self.flashing = Some(row);
        self.generation
    }

    pub fn copy_expired(&mut self, generation: u64) {
        if generation == self.generation {
            self.flashing = None;
        }
    }
}

pub fn view<'a>(
    conversation: &'a Conversation,
    state: State,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let is_group = conversation.kind == Kind::Group;

    let header = row![
        text("DETAILS")
            .size(theme::TEXT_SIZE - 2.0)
            .style(theme::text::secondary)
            .font_maybe(theme::font_style::secondary(theme).map(font::get)),
        Space::new().width(Length::Fill),
        button(icon::cancel().size(theme::TEXT_SIZE - 2.0))
            .padding(2)
            .style(theme::button::bare)
            .on_press(Message::Close),
    ]
    .align_y(iced::Alignment::Center);

    let description = conversation.description.as_deref().map(|description| {
        let body = text(description)
            .style(theme::text::secondary)
            .font_maybe(theme::font_style::secondary(theme).map(font::get))
            .ellipsis(iced::widget::text::Ellipsis::End);

        if description.chars().count() > DESCRIPTION_CLAMP_CHARS {
            body.height(Length::Fixed(DESCRIPTION_MAX_HEIGHT))
        } else {
            body
        }
    });

    let kind_row = detail_row(
        "Type",
        if is_group { "Group" } else { "Direct" }.to_string(),
        None,
        state,
        theme,
    );

    let members_row = is_group.then(|| {
        detail_row("Members", member_summary(conversation), None, state, theme)
    });

    let address_row = conversation.peer_address().map(|address| {
        detail_row(
            "Address",
            address.short_label().to_string(),
            Some((Row::Address, address.as_str().to_string())),
            state,
            theme,
        )
    });

    let convo_row = detail_row(
        "Conversation",
        conversation.id.short_label().to_string(),
        Some((Row::ConvoId, conversation.id.to_string())),
        state,
        theme,
    );

    container(
        column![
            header,
            description,
            kind_row,
            members_row,
            address_row,
            convo_row
        ]
        .spacing(6),
    )
    .width(Length::Fill)
    .padding(padding::all(4).left(8))
    .into()
}

/// One key/value line; `copy` adds a button that copies the full
/// (untruncated) value and confirms with a short "Copied" flash.
fn detail_row<'a>(
    key: &'a str,
    value: String,
    copy: Option<(Row, String)>,
    state: State,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let copy_action = copy.map(|(row, full_value)| {
        if state.flashing == Some(row) {
            Element::from(
                row![
                    icon::checkmark().size(theme::TEXT_SIZE - 2.0),
                    text("Copied")
                        .size(theme::TEXT_SIZE - 2.0)
                        .style(theme::text::success)
                        .font_maybe(
                            theme::font_style::secondary(theme).map(font::get),
                        ),
                ]
                .spacing(2)
                .align_y(iced::Alignment::Center),
            )
        } else {
            button(icon::copy().size(theme::TEXT_SIZE - 2.0))
                .padding(2)
                .style(theme::button::bare)
                .on_press(Message::Copy(row, full_value))
                .into()
        }
    });

    row![
        text(key)
            .style(theme::text::secondary)
            .font_maybe(theme::font_style::secondary(theme).map(font::get))
            .width(Length::Fixed(84.0)),
        text(value)
            .style(theme::text::primary)
            .font(font::MONO.clone()),
        copy_action,
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center)
    .into()
}

/// The roster's size with invitations named separately: an invited
/// member does not count towards the group until they join.
fn member_summary(conversation: &Conversation) -> String {
    let summary = format!("{} joined", conversation.joined_member_count());
    let pending = conversation.pending_member_count();

    if pending == 0 {
        summary
    } else {
        format!("{summary}, {pending} invited")
    }
}
