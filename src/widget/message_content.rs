use data::appearance::theme::FontStyle;
use data::{Config, message};
use iced::widget::span;
use iced::widget::text::Span;
use unicode_segmentation::UnicodeSegmentation;

use super::{Element, Renderer, selectable_rich_text, selectable_text};
use crate::{Theme, font};

/// Clickable fragments inside rendered message content. The IRC-era link
/// taxonomy (users, channels, go-to-message) is gone; only URLs remain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Url(String),
}

impl selectable_rich_text::Link for Link {
    fn underline(&self) -> bool {
        match self {
            Link::Url(_) => true,
        }
    }
}

pub fn message_content<'a, M: 'a + std::clone::Clone>(
    content: &'a message::Content,
    theme: &'a Theme,
    on_link: impl Fn(Link) -> M + 'a,
    style: impl Fn(&Theme) -> selectable_text::Style + 'a,
    font_style: impl Fn(&Theme) -> Option<FontStyle>,
    config: &Config,
) -> Element<'a, M> {
    match content {
        // A module log line renders through `buffer::log_row`, which styles
        // level, target and message separately from the parsed record. There
        // is no fragment parsing to do: the text is a terminal line, not a
        // chat message.
        message::Content::ModuleLog(record) => selectable_text(&record.message)
            .font_maybe(font_style(theme).map(font::get))
            .style(style)
            .into(),
        message::Content::Plain(text) => {
            let selectable_text = if let Some(only_emojis_size) =
                config.font.only_emojis_size
                && !text.is_empty()
                && UnicodeSegmentation::graphemes(text.as_str(), true)
                    .all(|grapheme| emojis::get(grapheme).is_some())
            {
                selectable_text(text)
                    .font_maybe(font_style(theme).map(font::get))
                    .size(f32::from(only_emojis_size))
                    .style(style)
            } else {
                selectable_text(text)
                    .font_maybe(font_style(theme).map(font::get))
                    .style(style)
            };

            selectable_text.into()
        }
        message::Content::Fragments(fragments) => {
            selectable_rich_text::<M, Link, (), Theme, Renderer>(
                fragments
                    .iter()
                    .map(|fragment| match fragment {
                        message::Fragment::Text(s) => span(s.as_str()),
                        message::Fragment::Url(u, s) => if config
                            .display
                            .decode_urls
                        {
                            span(data::url::display(u))
                        } else {
                            span(s.as_str())
                        }
                        .font_maybe(
                            theme.styles().buffer.url.font_style.map(font::get),
                        )
                        .color(theme.styles().buffer.url.color)
                        .link(Link::Url(u.as_str().to_string())),
                    })
                    .collect::<Vec<_>>(),
            )
            .on_link(on_link)
            .font_maybe(font_style(theme).map(font::get))
            .style(style)
            .into()
        }
        message::Content::Log(record) => {
            let spans: Vec<Span<'a, Link, _>> = vec![
                span(&record.message)
                    .font_maybe(font_style(theme).map(font::get)),
            ];

            selectable_rich_text::<M, Link, (), Theme, Renderer>(spans)
                .style(style)
                .into()
        }
    }
}
