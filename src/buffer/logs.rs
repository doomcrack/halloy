use data::{Config, history, message};
use iced::widget::{container, row};
use iced::{Length, Size, Task};

use super::{context_menu, scroll_view};
use crate::widget::{Element, selectable_text};
use crate::{Theme, font, theme};

#[derive(Debug, Clone)]
pub enum Message {
    ScrollView(scroll_view::Message),
}

pub enum Event {
    ContextMenu(context_menu::Event),
    MarkAsRead,
    OpenUrl(String),
}

pub fn view<'a>(
    state: &'a Logs,
    history: &'a history::Manager,
    config: &'a Config,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let messages = container(
        scroll_view::view(
            &state.scroll_view,
            scroll_view::Kind::Logs,
            history,
            config,
            theme,
            move |message: &'a data::Message, _, _, _| match message
                .target
                .source()
            {
                message::Source::Internal(message::source::Internal::Logs(
                    level,
                )) => {
                    let timestamp = config
                        .buffer
                        .format_timestamp(&message.server_time)
                        .map(|timestamp| {
                            selectable_text(timestamp)
                                .style(theme::selectable_text::timestamp)
                                .font_maybe(
                                    theme::font_style::timestamp(theme)
                                        .map(font::get),
                                )
                        });

                    let log_level_style = move |message_theme: &Theme| {
                        theme::selectable_text::log_level(message_theme, *level)
                    };
                    let log_level = selectable_text(
                        // Infer left or right alignment preference from
                        // sender alignment setting
                        if config.buffer.sender.alignment.is_right() {
                            format!("{level: >5}")
                        } else {
                            format!("{level: <5}")
                        },
                    )
                    .style(log_level_style)
                    .font_maybe(
                        theme::font_style::log_level(theme, *level)
                            .map(font::get),
                    );

                    let message = selectable_text(message.text())
                        .font_maybe(
                            theme::font_style::primary(theme).map(font::get),
                        )
                        .style(theme::selectable_text::logs);

                    Some(
                        row![
                            timestamp,
                            selectable_text(" "),
                            log_level,
                            selectable_text(" "),
                            message,
                        ]
                        .into(),
                    )
                }
                _ => None,
            },
        )
        .map(Message::ScrollView),
    )
    .height(Length::Fill);

    container(messages)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(8)
        .into()
}

#[derive(Debug, Clone)]
pub struct Logs {
    pub scroll_view: scroll_view::State,
}

impl Logs {
    pub fn new(pane_size: Size, config: &Config) -> Self {
        Self {
            scroll_view: scroll_view::State::new(pane_size, config),
        }
    }

    pub fn update(
        &mut self,
        message: Message,
        history: &mut history::Manager,
        config: &Config,
    ) -> (Task<Message>, Option<Event>) {
        match message {
            Message::ScrollView(message) => {
                let (command, event) = self.scroll_view.update(
                    message,
                    scroll_view::Kind::Logs,
                    history,
                    config,
                );

                let event = event.map(|event| match event {
                    scroll_view::Event::ContextMenu(event) => {
                        Event::ContextMenu(event)
                    }
                    scroll_view::Event::MarkAsRead => Event::MarkAsRead,
                    scroll_view::Event::OpenUrl(url) => Event::OpenUrl(url),
                });

                (command.map(Message::ScrollView), event)
            }
        }
    }
}
