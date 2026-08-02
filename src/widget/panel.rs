//! Dense read-only primitives for an operator panel.
//!
//! Nothing here knows what a blockchain is. The blockchain panel is the
//! first caller, but a delivery or capability panel wants the same six
//! shapes, so each takes plain strings and a [`Tone`] rather than a domain
//! type.
//!
//! Two rules the blockchain work established, worth keeping if these are
//! reused:
//!
//! - **A missing value is not an empty value.** [`stat`] takes an
//!   `Option<&str>` plus a note for the `None` case, so "this build has no
//!   peer count" and "the peer count is zero" cannot render the same.
//! - **Density over decoration.** These are read at a glance while
//!   something is going wrong; there is no motion and no card chrome.

use iced::widget::{Space, column, container, row, text};
use iced::{Alignment, Length};

use super::Element;
use crate::{Theme, font, theme};

/// How a value should read at a glance. Deliberately about *meaning*, not
/// colour: the caller says what the value is, the theme decides how it
/// looks, and a future high-contrast theme changes one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Neutral,
    /// Working as intended.
    Good,
    /// Working, but not finished — a bootstrap, a lag, a retry.
    Working,
    /// Wrong, and worth acting on.
    Bad,
    /// Not available at all, and never will be on this build.
    Absent,
}

impl Tone {
    fn style(self) -> fn(&Theme) -> iced::widget::text::Style {
        match self {
            Self::Neutral => theme::text::primary,
            Self::Good => theme::text::success,
            Self::Working => theme::text::secondary,
            Self::Bad => theme::text::error,
            Self::Absent => theme::text::tertiary,
        }
    }
}

/// A short status word, for a header. `[Online]`, `[Bootstrapping]`.
pub fn chip<'a, M: 'a>(
    label: impl Into<String>,
    tone: Tone,
    theme: &Theme,
) -> Element<'a, M> {
    container(
        text(label.into())
            .size(11)
            .style(tone.style())
            .font_maybe(theme::font_style::primary(theme).map(font::get)),
    )
    .padding([1, 6])
    .into()
}

/// One labelled figure.
///
/// `value` is `None` when there is nothing to show, and `note` says why —
/// "not in this build", "nothing finalised yet". The note renders in the
/// value's place, dimmed, so a reader can tell absence from zero without
/// hunting for it.
pub fn stat<'a, M: 'a>(
    label: impl Into<String>,
    value: Option<String>,
    note: Option<&str>,
    tone: Tone,
    theme: &Theme,
) -> Element<'a, M> {
    let (shown, tone) = match value {
        Some(value) => (value, tone),
        None => (note.unwrap_or("—").to_owned(), Tone::Absent),
    };

    column![
        text(label.into())
            .size(10)
            .style(theme::text::tertiary)
            .font_maybe(theme::font_style::secondary(theme).map(font::get)),
        text(shown)
            .size(13)
            .style(tone.style())
            .font_maybe(theme::font_style::primary(theme).map(font::get)),
    ]
    .spacing(1)
    .into()
}

/// Stats laid out in even columns, wrapping by row.
///
/// `per_row` rather than a responsive layout because the caller knows what
/// belongs together — mode beside height, tip beside lib — and a reflow
/// that splits a pair costs more than the space it saves.
pub fn stat_grid<'a, M: 'a>(
    stats: Vec<Element<'a, M>>,
    per_row: usize,
) -> Element<'a, M> {
    let mut rows = Vec::new();
    let mut current: Vec<Element<'a, M>> = Vec::new();

    for stat in stats {
        current.push(container(stat).width(Length::Fill).into());

        if current.len() == per_row.max(1) {
            rows.push(row(std::mem::take(&mut current)).spacing(12).into());
        }
    }

    if !current.is_empty() {
        // The last row is padded so its columns line up with the rows
        // above rather than stretching to fill.
        while current.len() < per_row.max(1) {
            current.push(Space::new().width(Length::Fill).into());
        }

        rows.push(row(current).spacing(12).into());
    }

    column(rows).spacing(8).into()
}

/// One cell of a [`table`] row.
///
/// There is no monospace flag: the whole app already runs on a mono
/// default font (`main.rs`), so hashes and ids align without asking.
pub struct Cell {
    pub text: String,
    /// Share of the row's width. Cells are proportional, not fixed: the
    /// pane is user-resizable and a fixed column either clips or strands
    /// space.
    pub portion: u16,
    pub tone: Tone,
}

impl Cell {
    pub fn new(text: impl Into<String>, portion: u16) -> Self {
        Self {
            text: text.into(),
            portion,
            tone: Tone::Neutral,
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }
}

/// A dense table. `headers` may be empty for an unlabelled list.
pub fn table<'a, M: 'a>(
    headers: &[Cell],
    rows: &[Vec<Cell>],
    theme: &Theme,
) -> Element<'a, M> {
    let line = |cells: &[Cell], header: bool| -> Element<'a, M> {
        row(cells.iter().map(|cell| {
            let content = text(cell.text.clone())
                .size(if header { 10 } else { 12 })
                .style(if header {
                    theme::text::tertiary
                } else {
                    cell.tone.style()
                })
                .font_maybe(theme::font_style::secondary(theme).map(font::get));

            container(content)
                .width(Length::FillPortion(cell.portion.max(1)))
                .into()
        }))
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    };

    let mut lines = Vec::new();

    if !headers.is_empty() {
        lines.push(line(headers, true));
    }

    lines.extend(rows.iter().map(|cells| line(cells, false)));

    column(lines).spacing(3).width(Length::Fill).into()
}

/// A line of prose standing in for content that is not there.
///
/// Used where a table or a figure would be. Says why, not just that.
pub fn absent<'a, M: 'a>(
    why: impl Into<String>,
    theme: &Theme,
) -> Element<'a, M> {
    text(why.into())
        .size(11)
        .style(theme::text::tertiary)
        .font_maybe(theme::font_style::secondary(theme).map(font::get))
        .into()
}
