use data::Config;
use data::message::Source;
use iced::Length;
use iced::widget::button;

use crate::widget::{Element, context_menu, text};
use crate::{Theme, font, theme};

#[derive(Debug, Clone, Copy)]
pub enum Entry {
    CopyMessage,
    CopySenderAddress,
    DmSender,
}

impl Entry {
    pub fn message_list(source: &Source) -> Vec<Self> {
        match source {
            Source::Peer(_) => {
                vec![
                    Entry::CopyMessage,
                    Entry::CopySenderAddress,
                    Entry::DmSender,
                ]
            }
            Source::Yourself | Source::Status(_) => vec![Entry::CopyMessage],
            Source::Internal(_) => vec![],
        }
    }

    pub fn view<'a>(
        self,
        message: &'a data::Message,
        length: Length,
        config: &'a Config,
        theme: &'a Theme,
    ) -> Element<'a, Message> {
        match self {
            Entry::CopyMessage => menu_button(
                "Copy message".to_string(),
                Some(Message::CopyText(message.text().into_owned())),
                length,
                theme,
                config,
            ),
            Entry::CopySenderAddress => menu_button(
                "Copy sender address".to_string(),
                message.target.source().peer().map(|address| {
                    Message::CopyText(address.as_str().to_string())
                }),
                length,
                theme,
                config,
            ),
            Entry::DmSender => menu_button(
                "DM sender".to_string(),
                message.target.source().peer().map(|address| {
                    Message::DmWith(address.as_str().to_string())
                }),
                length,
                theme,
                config,
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    CopyText(String),
    DmWith(String),
}

#[derive(Debug, Clone)]
pub enum Event {
    CopyText(String),
    DmWith(String),
}

pub fn update(message: Message) -> Event {
    match message {
        Message::CopyText(text) => Event::CopyText(text),
        Message::DmWith(address) => Event::DmWith(address),
    }
}

/// Wraps a rendered message row in its right-click menu. Rows without
/// menu entries (log records) are returned untouched.
pub fn message<'a, M>(
    content: impl Into<Element<'a, M>>,
    message: &'a data::Message,
    config: &'a Config,
    theme: &'a Theme,
) -> Element<'a, M>
where
    M: From<Message> + 'a,
{
    let entries = Entry::message_list(message.target.source());

    if entries.is_empty() {
        return content.into();
    }

    context_menu(
        context_menu::MouseButton::default(),
        context_menu::Anchor::Cursor,
        context_menu::ToggleBehavior::KeepOpen,
        None,
        content,
        entries,
        move |entry, length| {
            entry.view(message, length, config, theme).map(M::from)
        },
    )
    .into()
}

fn menu_button(
    content: String,
    message: Option<Message>,
    length: Length,
    theme: &Theme,
    config: &Config,
) -> Element<'static, Message> {
    let text_style = if message.is_some() {
        theme::text::primary
    } else {
        theme::text::secondary
    };

    button(
        text(content)
            .style(text_style)
            .font_maybe(theme::font_style::primary(theme).map(font::get)),
    )
    .padding(config.context_menu.padding.entry)
    .width(length)
    .on_press_maybe(message)
    .into()
}
