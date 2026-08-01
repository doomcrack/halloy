//! Persistent status strip along the dashboard's foot: backend errors
//! queue here (newest shown with a count, click to clear), the delivery
//! state is spelled out whenever it is not `Online`, and a backend that
//! restarted under the user says so until they dismiss it — the QML
//! feat/status-bar direction transposed onto halloy chrome.

use data::delivery::DeliveryState;
use iced::widget::{Space, button, container, row, text};
use iced::{Length, padding};

use crate::widget::Element;
use crate::{Theme, font, icon, theme};

#[derive(Debug, Clone)]
pub enum Message {
    ClearErrors,
    AcknowledgeRestarts(u32),
}

/// The dashboard-owned error queue. Only the newest error is displayed;
/// the rest fold into a count so a burst of failures stays one strip.
#[derive(Debug, Clone, Default)]
pub struct StatusBar {
    errors: Vec<String>,
    /// How many backend restarts the user has already been told about.
    ///
    /// The count itself belongs to the session, which is where the phases
    /// land; what is dismissible is the *notice*, and a restart after the
    /// dismissal has to raise it again. Comparing counts says both with one
    /// number and no subscription to keep in step.
    acknowledged_restarts: u32,
}

impl StatusBar {
    pub fn push_error(&mut self, error: String) {
        self.errors.push(error);
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::ClearErrors => self.errors.clear(),
            Message::AcknowledgeRestarts(count) => {
                self.acknowledged_restarts = count;
            }
        }
    }
}

/// The strip, or `None` while there is nothing to say (online, no errors,
/// no unacknowledged restart) so the dashboard loses no height to it.
pub fn view<'a>(
    state: &'a StatusBar,
    session: &'a data::Session,
    theme: &'a Theme,
) -> Option<Element<'a, Message>> {
    let delivery = delivery_label(session);
    let newest_error = state.errors.last();
    let unacknowledged_restarts =
        session.restarts.saturating_sub(state.acknowledged_restarts);

    if delivery.is_none()
        && newest_error.is_none()
        && unacknowledged_restarts == 0
    {
        return None;
    }

    let delivery = delivery.map(|(label, is_error)| {
        text(label)
            .style(if is_error {
                theme::text::error
            } else {
                theme::text::secondary
            })
            .font_maybe(theme::font_style::secondary(theme).map(font::get))
    });

    // Quiet on purpose, and deliberately not an error: by the time it is
    // readable the backend is back. What it buys the user is the ability to
    // account for what they just watched — every module going away and
    // coming back, a pane's log starting over from nothing — instead of
    // being left to conclude the app is unreliable.
    let restarted = (unacknowledged_restarts > 0).then(|| {
        let label = match session.restarts {
            1 => "The backend restarted".to_owned(),
            count => format!("The backend restarted {count} times"),
        };

        button(
            row![
                text(label).style(theme::text::secondary).font_maybe(
                    theme::font_style::secondary(theme).map(font::get),
                ),
                text("· click to clear")
                    .style(theme::text::tertiary)
                    .font_maybe(
                        theme::font_style::tertiary(theme).map(font::get),
                    ),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .padding([0, 4])
        .style(theme::button::bare)
        .on_press(Message::AcknowledgeRestarts(session.restarts))
    });

    let error = newest_error.map(|error| {
        let label = if state.errors.len() > 1 {
            format!("{error} (+{} more)", state.errors.len() - 1)
        } else {
            error.clone()
        };

        button(
            row![
                    icon::not_sent().width(14).height(14),
                    text(label).style(theme::text::error).font_maybe(
                        theme::font_style::error(theme).map(font::get),
                    ),
                    text("· click to clear")
                        .style(theme::text::tertiary)
                        .font_maybe(
                            theme::font_style::tertiary(theme).map(font::get),
                        ),
                ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .padding([0, 4])
        .style(theme::button::bare)
        .on_press(Message::ClearErrors)
    });

    Some(
        container(
            row![delivery, restarted, Space::new().width(Length::Fill), error,]
                .spacing(8)
                .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .padding(padding::all(4).left(8).right(8))
        .style(theme::container::buffer_title_bar)
        .into(),
    )
}

/// What to say about delivery while actions are gated; `None` while
/// online (the strip then only appears if errors are queued).
fn delivery_label(session: &data::Session) -> Option<(String, bool)> {
    let detail = &session.delivery.detail;

    match session.delivery.status {
        DeliveryState::Online => None,
        DeliveryState::Initialising | DeliveryState::Unknown => Some((
            if detail.is_empty() {
                "Connecting...".to_string()
            } else {
                format!("{detail}...")
            },
            false,
        )),
        DeliveryState::Error => Some((
            if detail.is_empty() {
                "Delivery error".to_string()
            } else {
                format!("Delivery error: {detail}")
            },
            true,
        )),
        DeliveryState::Stopped => Some(("Delivery stopped".to_string(), true)),
    }
}
