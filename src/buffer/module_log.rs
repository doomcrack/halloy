//! One module's log, as a read-only pane.
//!
//! **There is no composer and no way to grow one.** `input_view` is
//! constructed in exactly one place — `Buffer::Conversation` — and this state
//! carries no draft, no input id and no send event, so a composer here would
//! have nothing to talk to. That is the structural half of "read-only"; the
//! other half is that [`Event`] has no variant that could reach the backend.
//! Everything this pane can produce is a scroll, a selection or a link.
//!
//! Highlighting is driven by the parsed
//! [`Record`](data::module::log::Record) that the tailer already produced:
//! level picks the colour, `target` renders dimmed ahead of the message. The
//! view never looks at raw text, so a dialect the parser learns later styles
//! correctly here for free.

use data::module::{self, ModuleId};
use data::{Config, history, message};
use iced::widget::{center, column, container, text};
use iced::{Length, Size, Task};

use super::{context_menu, log_row, scroll_view};
use crate::widget::Element;
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
    state: &'a ModuleLog,
    module: Option<&'a data::Module>,
    history: &'a history::Manager,
    config: &'a Config,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let kind = history::Kind::Module(state.module.clone());

    if let Some(placeholder) = placeholder(state, module, history, &kind, theme)
    {
        return placeholder;
    }

    let messages = container(
        scroll_view::view(
            &state.scroll_view,
            scroll_view::Kind::Module(&state.module),
            history,
            config,
            theme,
            move |message: &'a data::Message, _, _, _| {
                let message::Source::Internal(
                    message::source::Internal::Module(level),
                ) = message.target.source()
                else {
                    return None;
                };

                // The record is the source of truth for everything styled;
                // a row whose content is not one is a message that does not
                // belong on this pane at all.
                let target = match &message.content {
                    data::message::Content::ModuleLog(record) => {
                        record.target.as_deref()
                    }
                    _ => None,
                };

                Some(log_row::view(
                    &message.server_time,
                    (*level).into(),
                    target,
                    message.text(),
                    config,
                    theme,
                ))
            },
        )
        .map(Message::ScrollView),
    )
    .height(Length::Fill);

    let banner = (module.map(|module| &module.status)
        == Some(&module::Status::Crashed))
    .then(|| crash_banner(state, theme));

    container(column![banner, messages])
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(8)
        .into()
}

/// What to show instead of an empty scrollback.
///
/// A module log has no backlog to fetch, so "nothing here" is a fact about
/// the world rather than a load in progress — and the four worlds it can mean
/// are worth telling apart. A crashed module never lands here: its lines are
/// the reason to look, so it keeps the log and gets a banner over it.
fn placeholder<'a>(
    state: &'a ModuleLog,
    module: Option<&'a data::Module>,
    history: &'a history::Manager,
    kind: &history::Kind,
    theme: &'a Theme,
) -> Option<Element<'a, Message>> {
    if !history.is_empty(kind) {
        return None;
    }

    let name = state.module.display_name();

    let message = match module.map(|module| &module.status) {
        // Before the first `listModules` report there is no row at all, and
        // the daemon may not even be up. Matches the sidebar's wording for
        // the same moment.
        None => "Waiting for the daemon…".to_owned(),
        Some(module::Status::Crashed) => {
            format!("{name} crashed before it logged anything")
        }
        Some(module::Status::Loaded) => "No log output yet".to_owned(),
        Some(module::Status::NotLoaded) => format!("{name} is not loaded"),
        Some(module::Status::Unknown(raw)) => {
            format!("{name} is {raw}")
        }
    };

    Some(
        center(
            text(message)
                .style(theme::text::secondary)
                .font_maybe(theme::font_style::secondary(theme).map(font::get)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
    )
}

/// Said over the log rather than instead of them: the lines above the abort
/// are the whole reason to open a crashed module's pane, and an idle-looking
/// empty pane is precisely the failure `logos-modules.md` §5 describes.
fn crash_banner<'a>(
    state: &'a ModuleLog,
    theme: &'a Theme,
) -> Element<'a, Message> {
    container(
        text(format!(
            "{} crashed — the log below ends where it died",
            state.module.display_name()
        ))
        .style(theme::text::error)
        .font_maybe(theme::font_style::primary(theme).map(font::get)),
    )
    .padding([2, 0])
    .into()
}

#[derive(Debug, Clone)]
pub struct ModuleLog {
    pub module: ModuleId,
    pub scroll_view: scroll_view::State,
}

impl ModuleLog {
    pub fn new(module: ModuleId, pane_size: Size, config: &Config) -> Self {
        Self {
            module,
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
                    scroll_view::Kind::Module(&self.module),
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
