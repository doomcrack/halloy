use data::{Config, history, message};
use iced::widget::container;
use iced::{Length, Size, Task};

use super::{context_menu, log_row, scroll_view};
use crate::Theme;
use crate::widget::Element;

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
                )) => Some(log_row::view(
                    &message.server_time,
                    *level,
                    None,
                    message.text(),
                    config,
                    theme,
                )),
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
