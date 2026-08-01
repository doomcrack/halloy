//! The one row layout every log-shaped buffer uses: a timestamp, a
//! fixed-width severity, and the line itself.
//!
//! Extracted from [`super::logs`] when the module monitor gained a second
//! caller. It exists to keep the two panes looking like the same terminal —
//! the severity ramp, the 5-wide level column and the alignment-aware padding
//! are all things that would drift the moment they were written twice — not
//! to be a general widget. It takes exactly what the two callers differ on and
//! nothing else.

use std::borrow::Cow;

use chrono::{DateTime, Utc};
use data::Config;
use data::log::Level;
use iced::widget::row;

use super::scroll_view;
use crate::widget::{Element, selectable_text};
use crate::{Theme, font, theme};

/// One log line.
///
/// `target` is the `tracing`-style origin of the line
/// (`logos_blockchain::storage`); it renders dimmed ahead of the message so
/// the eye can skip it, and is absent for dialects that carry none.
pub fn view<'a>(
    server_time: &DateTime<Utc>,
    level: Level,
    target: Option<&'a str>,
    message: Cow<'a, str>,
    config: &'a Config,
    theme: &'a Theme,
) -> Element<'a, scroll_view::Message> {
    let timestamp =
        config
            .buffer
            .format_timestamp(server_time)
            .map(|timestamp| {
                selectable_text(timestamp)
                    .style(theme::selectable_text::timestamp)
                    .font_maybe(
                        theme::font_style::timestamp(theme).map(font::get),
                    )
            });

    let level_style = move |message_theme: &Theme| {
        theme::selectable_text::log_level(message_theme, level)
    };

    let message_style = move |message_theme: &Theme| {
        theme::selectable_text::log_message(message_theme, level)
    };

    // Infer left or right alignment preference from sender alignment setting
    let level = selectable_text(if config.buffer.sender.alignment.is_right() {
        format!("{level: >5}")
    } else {
        format!("{level: <5}")
    })
    .style(level_style)
    .font_maybe(theme::font_style::log_level(theme, level).map(font::get));

    let target = target.map(|target| {
        selectable_text(format!("{target}: "))
            .style(theme::selectable_text::timestamp)
            .font_maybe(theme::font_style::timestamp(theme).map(font::get))
    });

    let message = selectable_text(message)
        .font_maybe(theme::font_style::primary(theme).map(font::get))
        .style(message_style);

    row![
        timestamp,
        selectable_text(" "),
        level,
        selectable_text(" "),
        target,
        message,
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use data::appearance::theme::{FontStyle, OptionalTextStyle, Styles};
    use iced::Color;

    use super::*;

    /// Everything [`view`] derives from the level alone: the word in the
    /// severity column, that column's colour and weight, and the colour of
    /// the message beside it. Two levels that agree on all four are two
    /// levels that render identically.
    fn styling(
        theme: &Theme,
        level: Level,
    ) -> (String, Option<Color>, Option<FontStyle>, Option<Color>) {
        (
            level.to_string(),
            theme::selectable_text::log_level(theme, level).color,
            theme::font_style::log_level(theme, level),
            theme::selectable_text::log_message(theme, level).color,
        )
    }

    /// The finding this answers: a module abort (`logos-modules.md` §5) reached the
    /// pane as `Level::Error` and was drawn as one, so the single line the
    /// monitor exists to surface was the hardest one to pick out of the
    /// errors around it.
    ///
    /// Asserted component by component rather than on the tuple as a whole:
    /// the word alone differing would satisfy a wholesale `assert_ne!` while
    /// leaving the colour folded back, which is most of what the finding was.
    #[test]
    fn a_crash_row_does_not_render_like_an_error_row() {
        let theme = Theme::default();

        let (word, level_color, weight, message_color) =
            styling(&theme, Level::Critical);
        let (error_word, error_color, error_weight, error_message) =
            styling(&theme, Level::Error);

        assert_ne!(word, error_word);
        assert_ne!(level_color, error_color);
        assert_ne!(weight, error_weight);
        assert_ne!(message_color, error_message);
    }

    /// The severity column is laid out with `{level:<5}`, so a level whose
    /// word is longer than five characters silently widens the gutter of
    /// every row in both panes.
    #[test]
    fn every_level_fits_the_five_wide_severity_column() {
        for level in [
            Level::Critical,
            Level::Error,
            Level::Warn,
            Level::Info,
            Level::Debug,
            Level::Trace,
        ] {
            let word = level.to_string();

            assert!(
                word.chars().count() <= 5,
                "{word} does not fit the severity column",
            );
        }
    }

    /// A theme written before the level existed sets no `text.critical`, and
    /// falling back to `text.error`'s colour is deliberate — a crash should
    /// read as at least an error. The weight is what has to carry the
    /// distinction there, so it is pinned separately from the colour.
    #[test]
    fn a_theme_that_never_heard_of_critical_still_tells_it_apart() {
        let mut styles = Styles::default();
        styles.text.critical = OptionalTextStyle::default();

        let theme = Theme::from(data::Theme::new("bare".to_owned(), styles));

        assert_eq!(
            theme::selectable_text::log_level(&theme, Level::Critical).color,
            theme::selectable_text::log_level(&theme, Level::Error).color,
            "the documented colour fallback stopped being the error colour",
        );
        assert_eq!(
            theme::font_style::log_level(&theme, Level::Critical),
            Some(FontStyle::Bold),
        );
        assert_ne!(
            styling(&theme, Level::Critical),
            styling(&theme, Level::Error),
        );
    }
}
