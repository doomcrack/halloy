use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, Utc};
use data::buffer::RightAlignmentWidths;
use data::config::buffer::HideConsecutiveEnabled;
use data::conversation::ConvoId;
use data::message::{self, Limit, Source};
use data::module::ModuleId;
use data::{Config, history};
use iced::widget::{
    self, Scrollable, column, container, row, rule, scrollable, sensor, space,
    text,
};
use iced::{Length, Size, Task, padding};
use tokio::time;

use self::correct_viewport::correct_viewport;
use self::keyed::keyed;
use super::context_menu;
use crate::widget::Element;
use crate::widget::message_content::Link;
use crate::{Theme, font, theme};

const SCROLL_TO_TIMEOUT: Duration = Duration::from_millis(200);
/// Pages of off-screen messages to keep rendered above and below the viewport
const BUFFER_PAGES: usize = 3;

const HIGHLIGHT_HOLD_MS: u64 = 2000;
const HIGHLIGHT_ALPHA_START: f32 = 1.0;
const HIGHLIGHT_ALPHA_TICK_MS: u64 = 20;
const HIGHLIGHT_ALPHA_STEP: f32 =
    HIGHLIGHT_ALPHA_START / (400.0 / HIGHLIGHT_ALPHA_TICK_MS as f32);

#[derive(Debug, Clone)]
pub enum Message {
    Scrolled {
        count: usize,
        has_more_older_messages: bool,
        has_more_newer_messages: bool,
        oldest: DateTime<Utc>,
        status: Status,
        viewport: scrollable::Viewport,
    },
    ContextMenu(context_menu::Message),
    Link(Link),
    ScrollTo(keyed::Hit),
    MarkAsRead,
    ContentResized(Size),
    PendingScrollTo,
    FadeHighlight(message::Hash, u64),
    HeightsCollected(Vec<(keyed::Key, f32)>),
}

impl From<context_menu::Message> for Message {
    fn from(message: context_menu::Message) -> Self {
        Message::ContextMenu(message)
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    ContextMenu(context_menu::Event),
    MarkAsRead,
    OpenUrl(String),
}

#[derive(Debug, Clone, Copy)]
pub enum Kind<'a> {
    Conversation(&'a ConvoId),
    /// One module's log. Borrowed like its conversation sibling — the kind is
    /// rebuilt on every view pass and cloning the id per frame would be the
    /// only allocation in the hot path.
    Module(&'a ModuleId),
    Logs,
}

impl From<Kind<'_>> for history::Kind {
    fn from(value: Kind<'_>) -> Self {
        match value {
            Kind::Conversation(convo_id) => {
                history::Kind::Conversation(convo_id.clone())
            }
            Kind::Module(module_id) => history::Kind::Module(module_id.clone()),
            Kind::Logs => history::Kind::Logs,
        }
    }
}

pub trait LayoutMessage<'a> {
    fn format(
        &self,
        message: &'a data::Message,
        right_alignment_widths: Option<RightAlignmentWidths>,
        hide_timestamp: bool,
        hide_sender: bool,
    ) -> Option<Element<'a, Message>>;

    /// Whether the layout names a sender only at the head of a run of
    /// theirs whatever `hide_consecutive` is set to. A conversation
    /// thread does (QML `sameSenderAsPrevious`), the log does not.
    fn collapses_sender_runs(&self) -> bool {
        false
    }
}

impl<'a, T> LayoutMessage<'a> for T
where
    T: Fn(
        &'a data::Message,
        Option<RightAlignmentWidths>,
        bool,
        bool,
    ) -> Option<Element<'a, Message>>,
{
    fn format(
        &self,
        message: &'a data::Message,
        right_alignment_widths: Option<RightAlignmentWidths>,
        hide_timestamp: bool,
        hide_sender: bool,
    ) -> Option<Element<'a, Message>> {
        self(message, right_alignment_widths, hide_timestamp, hide_sender)
    }
}

/// Consecutive-sender detection: same [`Source`] (Peer with equal address,
/// or Yourself) within the optional duration window.
fn is_consecutive_message(
    message: &data::Message,
    prev_message: Option<&data::Message>,
    duration: Option<chrono::TimeDelta>,
) -> bool {
    matches!(message.target.source(), Source::Peer(_) | Source::Yourself)
        && prev_message.is_some_and(|prev_message| {
            duration.is_none_or(|duration| {
                message.server_time - prev_message.server_time < duration
            }) && message.target.source() == prev_message.target.source()
        })
}

/// Whether a message continues the run its predecessor belongs to, mirroring
/// QML `MessageListModel::SameSenderAsPreviousRole`: same sender *and* the
/// same local calendar day, so the message heading a new day is always named
/// even when the day before ended with one of theirs.
fn continues_sender_run(
    message: &data::Message,
    prev_message: Option<&data::Message>,
    duration: Option<chrono::TimeDelta>,
) -> bool {
    is_consecutive_message(message, prev_message, duration)
        && prev_message.is_some_and(|prev_message| {
            message.server_time.with_timezone(&Local).date_naive()
                == prev_message.server_time.with_timezone(&Local).date_naive()
        })
}

/// Whether a message's sender label is suppressed. A layout that collapses
/// runs (a conversation thread, QML `startsRun`) names a sender only where
/// their run begins whatever the alignment; `hide_consecutive` then only
/// widens that to layouts which do not, and narrows it to a time window.
fn hide_sender(
    message: &data::Message,
    prev_message: Option<&data::Message>,
    hide_consecutive: HideConsecutiveEnabled,
    collapses_sender_runs: bool,
) -> bool {
    match hide_consecutive {
        HideConsecutiveEnabled::Enabled(duration) => {
            continues_sender_run(message, prev_message, duration)
        }
        HideConsecutiveEnabled::Disabled => {
            collapses_sender_runs
                && continues_sender_run(message, prev_message, None)
        }
    }
}

/// The visible width of a message's sender label, for right alignment.
pub fn sender_label(message: &data::Message) -> Option<&str> {
    match message.target.source() {
        Source::Peer(address) => Some(address.short_label()),
        Source::Yourself => Some("you"),
        Source::Status(_) | Source::Internal(_) => None,
    }
}

pub fn view<'a>(
    state: &State,
    kind: Kind,
    history: &'a history::Manager,
    config: &'a Config,
    theme: &'a Theme,
    formatter: impl LayoutMessage<'a> + 'a,
) -> Element<'a, Message> {
    let divider_font_size =
        config.font.size.map_or(theme::TEXT_SIZE, f32::from) - 1.0;

    let Some(history::View {
        has_more_older_messages,
        has_more_newer_messages,
        old_messages,
        new_messages,
        cleared: _,
        ..
    }) = history.get_messages(&kind.into(), Some(state.limit))
    else {
        return column![].into();
    };

    let count = old_messages.len() + new_messages.len();
    let oldest = old_messages
        .iter()
        .chain(&new_messages)
        .next()
        .map_or_else(Utc::now, |message| message.server_time);
    let status = state.status;

    let right_alignment_widths =
        config.buffer.sender.alignment.is_right().then_some({
            let max_timestamp_width = old_messages
                .iter()
                .chain(&new_messages)
                .filter_map(|message| timestamp_width(message, config))
                .fold(0.0, f32::max);

            let max_sender_width = old_messages
                .iter()
                .chain(&new_messages)
                .filter_map(|message| {
                    sender_label(message).map(|label| {
                        font::width_from_str(label, &config.font) + 1.0
                    })
                })
                .fold(0.0, f32::max);

            let message_marker_width =
                font::width_of_message_marker(&config.font) + 1.0;

            RightAlignmentWidths {
                prefixes: 0.0,
                timestamp: max_timestamp_width,
                middle: max_sender_width.max(message_marker_width),
            }
        });

    let message_rows = |last_date: Option<NaiveDate>,
                        messages: &[&'a data::Message]| {
        messages
            .iter()
            .scan(Option::<&data::Message>::None, |prev_message, message| {
                let hide_timestamp =
                    if let HideConsecutiveEnabled::Enabled(duration) =
                        config.buffer.timestamp.hide_consecutive.enabled
                    {
                        is_consecutive_message(message, *prev_message, duration)
                    } else {
                        false
                    };

                let hide_sender = hide_sender(
                    message,
                    *prev_message,
                    config.buffer.sender.hide_consecutive.enabled,
                    formatter.collapses_sender_runs(),
                );

                *prev_message = Some(message);

                Some(
                    formatter
                        .format(
                            message,
                            right_alignment_widths,
                            hide_timestamp,
                            hide_sender,
                        )
                        .map(|element| {
                            (
                                message,
                                context_menu::message(
                                    element, message, config, theme,
                                ),
                            )
                        }),
                )
            })
            .flatten()
            .scan(last_date, |last_date, (message, element)| {
                let date =
                    message.server_time.with_timezone(&Local).date_naive();

                let is_new_day = last_date.is_none_or(|prev| date > prev);

                *last_date = Some(date);

                let element = if let Some((hash, alpha)) =
                    state.highlighted_message
                    && hash == message.hash
                {
                    container(element)
                        .width(Length::Fill)
                        .style(move |theme| {
                            theme::container::highlighted_message(theme, alpha)
                        })
                        .into()
                } else {
                    element
                };

                let content =
                    if is_new_day && config.buffer.date_separators.show {
                        // The day a run belongs to rides on a chip floating
                        // over the thread, not a rule cutting it in two (QML
                        // `DayChip`); only dates the relative labels have no
                        // word for fall back to the configured format.
                        let label = data::time::day_chip_label(date)
                            .unwrap_or_else(|| {
                                config.buffer.format_date_separator(&date)
                            });

                        column![
                            container(
                                container(
                                    text(label)
                                        .size(divider_font_size)
                                        .style(theme::text::date_separator)
                                        .font_maybe(
                                            theme::font_style::secondary(theme)
                                                .map(font::get)
                                        )
                                )
                                .padding([2, 10])
                                .style(theme::container::day_chip)
                            )
                            .width(Length::Fill)
                            .padding(padding::top(4).bottom(4))
                            .align_x(iced::Alignment::Center),
                            element
                        ]
                        .into()
                    } else {
                        element
                    };

                Some(keyed(keyed::Key::message(message), content))
            })
            .collect::<Vec<_>>()
    };

    let line_spacing = config.buffer.line_spacing;

    // Only create widgets for messages near the viewport, use height
    // spacers for the rest so we doesn't lay out thousands of children
    let row_height =
        theme::resolve_line_height(&config.font) + line_spacing as f32;
    let total = old_messages.len() + new_messages.len();
    let visible = (state.pane_size.height / row_height).ceil() as usize;
    let buffer = visible * BUFFER_PAGES;
    let render_budget = visible + 2 * buffer;

    let msg_height = |m: &&data::Message| -> f32 {
        state
            .height_cache
            .get(&keyed::Key::Message(m.hash))
            .copied()
            .map_or(row_height, |h| h + line_spacing as f32)
    };
    let div_height = state
        .height_cache
        .get(&keyed::Key::Divider)
        .copied()
        .unwrap_or_default();

    let (render_start, render_end) = if state.pending_scroll_to.is_some()
        || state.is_scrolling_to
        || total <= render_budget
    {
        (0, total)
    } else {
        let first_visible = match state.status {
            Status::Bottom => {
                let offset = state.last_scroll_offset;
                let mut acc = 0.0_f32;
                let mut from_bottom = 0;
                for m in old_messages.iter().chain(&new_messages).rev() {
                    if from_bottom == new_messages.len() {
                        acc += div_height;
                        if acc > offset {
                            break;
                        }
                    }

                    acc += msg_height(m);
                    if acc > offset {
                        break;
                    }
                    from_bottom += 1;
                }
                total.saturating_sub(from_bottom + visible)
            }
            Status::Unlocked => {
                let offset = state.last_scroll_offset;
                let mut acc = 0.0_f32;
                let mut idx = 0;
                for m in old_messages.iter().chain(&new_messages) {
                    if idx == old_messages.len() {
                        acc += div_height;
                        if acc > offset {
                            break;
                        }
                    }

                    acc += msg_height(m);
                    if acc > offset {
                        break;
                    }
                    idx += 1;
                }
                idx
            }
        };

        (
            first_visible.saturating_sub(buffer),
            (first_visible + visible + buffer).min(total),
        )
    };

    let old_start = render_start.min(old_messages.len());
    let old_end = render_end.min(old_messages.len());
    let new_start = render_start
        .saturating_sub(old_messages.len())
        .min(new_messages.len());
    let new_end = render_end
        .saturating_sub(old_messages.len())
        .min(new_messages.len());

    let date_of =
        |m: &data::Message| m.server_time.with_timezone(&Local).date_naive();

    let old_last_date = old_start
        .checked_sub(1)
        .and_then(|i| old_messages.get(i))
        .map(|m| date_of(m));

    let new_last_date = new_start
        .checked_sub(1)
        .and_then(|i| new_messages.get(i))
        .map(|m| date_of(m))
        .or_else(|| old_messages.last().map(|m| date_of(m)));

    let old = message_rows(old_last_date, &old_messages[old_start..old_end]);
    let new = message_rows(new_last_date, &new_messages[new_start..new_end]);

    let top_spacer = (render_start > 0).then(|| {
        let h: f32 = old_messages[..old_start]
            .iter()
            .chain(&new_messages[..new_start])
            .map(&msg_height)
            .sum();
        space::vertical().height(h)
    });
    let bottom_spacer = (render_end < total).then(|| {
        let h: f32 = old_messages[old_end..]
            .iter()
            .chain(&new_messages[new_end..])
            .map(&msg_height)
            .sum();
        space::vertical().height(h)
    });

    let show_backlog_divider = if old.is_empty() {
        // If all newer messages in viewport, only show backlog divider at the top
        // if we don't have any older messages at all (we're scrolled all the way up)
        !has_more_older_messages
    } else {
        // Always show backlog divider after any visible older messages
        if config.buffer.backlog_separator.hide_when_all_read {
            !new_messages.is_empty()
        } else {
            true
        }
    };

    let divider = show_backlog_divider.then(|| {
        match &config.buffer.backlog_separator.text {
            data::buffer::BacklogText::Hidden => row![
                container(rule::horizontal(1).style(theme::rule::backlog))
                    .padding([2, 0])
                    .width(Length::Fill)
            ]
            .padding(2)
            .align_y(iced::Alignment::Center),
            data::buffer::BacklogText::Text(separator_text) => row![
                container(rule::horizontal(1).style(theme::rule::backlog))
                    .width(Length::Fill)
                    .padding(padding::right(6)),
                text(separator_text)
                    .size(divider_font_size)
                    .style(theme::text::backlog)
                    .font_maybe(
                        theme::font_style::secondary(theme).map(font::get)
                    ),
                container(rule::horizontal(1).style(theme::rule::backlog))
                    .width(Length::Fill)
                    .padding(padding::left(6))
            ]
            .padding(2)
            .align_y(iced::Alignment::Center),
        }
    });

    // Only push parts that render something. `Column::push` skips void
    // children, but an empty `row![]` or `column()` is not void, so it still
    // claims a `spacing(line_spacing)` gap. Once a buffer is marked as read the
    // divider and the `new` column both empty out, stranding that spacing at
    // the end as blank space above the input.
    let mut content_column = widget::Column::new().spacing(line_spacing);

    if let Some(top_spacer) = top_spacer {
        content_column = content_column.push(top_spacer);
    }

    if !old.is_empty() {
        content_column = content_column.push(column(old).spacing(line_spacing));
    }

    if let Some(divider) = divider {
        content_column =
            content_column.push(keyed(keyed::Key::Divider, divider));
    }

    if !new.is_empty() {
        content_column = content_column.push(column(new).spacing(line_spacing));
    }

    if let Some(bottom_spacer) = bottom_spacer {
        content_column = content_column.push(bottom_spacer);
    }

    let content =
        sensor(content_column.push(space::vertical().height(line_spacing)))
            .on_resize(Message::ContentResized);

    correct_viewport(
        Scrollable::new(container(content).width(Length::Fill).padding([0, 8]))
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::default()
                    .anchor(status.anchor())
                    .width(config.pane.scrollbar.width)
                    .scroller_width(config.pane.scrollbar.scroller_width),
            ))
            .on_scroll(move |viewport| Message::Scrolled {
                has_more_older_messages,
                has_more_newer_messages,
                count,
                oldest,
                status,
                viewport,
            })
            .id(state.scrollable.clone()),
        state.scrollable.clone(),
        matches!(state.status, Status::Unlocked),
    )
}

#[derive(Debug, Clone)]
pub struct State {
    pub scrollable: widget::Id,
    pane_size: Size,
    content_size: Size,
    limit: Limit,
    status: Status,
    last_scroll_offset: f32,
    height_cache: HashMap<keyed::Key, f32>,
    pending_scroll_to: Option<keyed::Key>,
    is_scrolling_to: bool,
    highlighted_message: Option<(message::Hash, f32)>,
    highlight_generation: u64,
}

impl State {
    pub fn new(pane_size: Size, config: &Config) -> Self {
        let step_messages = step_messages(2.0 * pane_size.height, config);

        Self {
            scrollable: widget::Id::unique(),
            pane_size,
            content_size: Size::default(), // Set initially by the content sensor.
            limit: Limit::Bottom(step_messages),
            status: Status::default(),
            last_scroll_offset: 0.0,
            height_cache: HashMap::new(),
            pending_scroll_to: None,
            is_scrolling_to: false,
            highlighted_message: None,
            highlight_generation: 0,
        }
    }

    pub fn update(
        &mut self,
        message: Message,
        kind: Kind,
        history: &mut history::Manager,
        config: &Config,
    ) -> (Task<Message>, Option<Event>) {
        match message {
            Message::Scrolled {
                count,
                has_more_older_messages,
                has_more_newer_messages,
                oldest,
                status: old_status,
                viewport,
            } => {
                if self.pending_scroll_to.is_some() || self.is_scrolling_to {
                    return (Task::none(), None);
                }

                self.last_scroll_offset = viewport.absolute_offset().y;

                let relative_offset = viewport.relative_offset().y;
                let absolute_offset = viewport.absolute_offset().y;
                let height = self.pane_size.height;

                let mut event = None;

                match old_status {
                    // Scrolling down from top & have more to load
                    _ if old_status.is_page_from_bottom(
                        absolute_offset,
                        height,
                        self.content_size.height,
                    ) && has_more_newer_messages =>
                    {
                        self.status = Status::Unlocked;
                        let n = count + step_messages(height, config);
                        self.limit = match self.limit {
                            Limit::Around(_, hash) => Limit::Around(n, hash),
                            _ => Limit::Top(n),
                        };
                    }
                    // Hit bottom, anchor it
                    _ if old_status.is_bottom(relative_offset) => {
                        if !matches!(self.status, Status::Bottom)
                            && config.buffer.mark_as_read.on_scroll_to_bottom
                        {
                            event = Some(Event::MarkAsRead);
                        }

                        self.status = Status::Bottom;

                        if matches!(self.limit, Limit::Bottom(_)) {
                            if old_status.is_page_from_top(
                                absolute_offset,
                                // Scale up page height to ensure that there
                                // isn't a simultaneous anchor flip and message
                                // load when scrolling up from bottom
                                2.0 * height,
                                self.content_size.height,
                            ) && has_more_older_messages
                            {
                                self.limit = Limit::Bottom(
                                    count + step_messages(height, config),
                                );
                            }
                        } else {
                            self.limit = Limit::Bottom(step_messages(
                                2.0 * height,
                                config,
                            ));
                        }
                    }
                    // Scrolling up from bottom & have more to load
                    _ if old_status.is_page_from_top(
                        absolute_offset,
                        height,
                        self.content_size.height,
                    ) && has_more_older_messages =>
                    {
                        self.status = Status::Unlocked;
                        let n = count + step_messages(height, config);

                        if let Limit::Around(_, hash) = self.limit {
                            self.limit = Limit::Around(n, hash);
                        } else {
                            self.limit = Limit::Bottom(n);

                            // Get new oldest message w/ new limit and use that w/ Since
                            if let Some(history::View {
                                old_messages,
                                new_messages,
                                ..
                            }) = history
                                .get_messages(&kind.into(), Some(self.limit))
                                && let Some(oldest) = old_messages
                                    .iter()
                                    .chain(&new_messages)
                                    .next()
                            {
                                self.limit = Limit::Since(oldest.server_time);
                            }
                        }
                    }
                    // Hit top, anchor it
                    _ if old_status.is_top(relative_offset) => {
                        self.status = Status::Unlocked;

                        if matches!(self.limit, Limit::Top(_)) {
                            if old_status.is_page_from_bottom(
                                absolute_offset,
                                height,
                                self.content_size.height,
                            ) && has_more_newer_messages
                            {
                                self.limit = Limit::Top(
                                    count + step_messages(height, config),
                                );
                            }
                        } else if matches!(self.limit, Limit::Around(_, _)) {
                            self.limit = Limit::Since(oldest);
                        } else {
                            self.limit =
                                Limit::Top(step_messages(2.0 * height, config));
                        }
                    }
                    // Move away from bottom
                    Status::Bottom
                        if !old_status.is_bottom(relative_offset) =>
                    {
                        self.status = Status::Unlocked;
                        self.limit = Limit::Since(oldest);
                    }
                    // Normal scrolling, always unlocked
                    _ => {
                        self.status = Status::Unlocked;

                        if !matches!(
                            self.limit,
                            Limit::Top(_) | Limit::Around(_, _)
                        ) {
                            self.limit = Limit::Since(oldest);
                        }
                    }
                }

                // If alignment changes, we need to flip the scrollable translation
                // for the new offset
                if let Some(new_offset) =
                    self.status.flipped(old_status, viewport)
                {
                    self.last_scroll_offset = new_offset.y;
                    let scroll_to = correct_viewport::scroll_to(
                        self.scrollable.clone(),
                        new_offset,
                    );
                    let collect =
                        keyed::collect_heights(self.scrollable.clone())
                            .map(Message::HeightsCollected);

                    return (Task::batch([scroll_to, collect]), event);
                }

                let collect = keyed::collect_heights(self.scrollable.clone())
                    .map(Message::HeightsCollected);

                return (collect, event);
            }
            Message::ContextMenu(message) => {
                return (
                    Task::none(),
                    Some(Event::ContextMenu(context_menu::update(message))),
                );
            }
            Message::Link(Link::Url(url)) => {
                return (Task::none(), Some(Event::OpenUrl(url)));
            }
            Message::ScrollTo(keyed::Hit {
                key,
                hit_bounds,
                scrollable,
            }) => {
                self.is_scrolling_to = false;

                let fade_task = if let keyed::Key::Message(hash) = key {
                    self.highlight_generation += 1;
                    let generation = self.highlight_generation;
                    self.highlighted_message =
                        Some((hash, HIGHLIGHT_ALPHA_START));
                    Task::perform(
                        time::sleep(Duration::from_millis(HIGHLIGHT_HOLD_MS)),
                        move |()| Message::FadeHighlight(hash, generation),
                    )
                } else {
                    Task::none()
                };

                let max_offset = scrollable.max_vertical_offset();

                let content_y = hit_bounds.y - scrollable.content.y;
                let viewport_top = scrollable.offset.y;
                let viewport_bottom =
                    scrollable.offset.y + scrollable.viewport.height;
                let is_visible = content_y >= viewport_top
                    && content_y + hit_bounds.height <= viewport_bottom;

                if is_visible {
                    return (fade_task, None);
                }

                let offset = content_y.max(0.0).min(max_offset);

                if (offset - max_offset).abs() <= f32::EPSILON {
                    self.status = Status::Bottom;

                    if !matches!(self.limit, Limit::Bottom(_)) {
                        self.limit = Limit::Bottom(step_messages(
                            2.0 * self.pane_size.height,
                            config,
                        ));
                    }

                    return (
                        Task::batch([
                            correct_viewport::scroll_to(
                                self.scrollable.clone(),
                                scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                            ),
                            fade_task,
                        ]),
                        None,
                    );
                } else {
                    self.status = Status::Unlocked;

                    return (
                        Task::batch([
                            correct_viewport::scroll_to(
                                self.scrollable.clone(),
                                scrollable::AbsoluteOffset {
                                    x: 0.0,
                                    y: offset,
                                },
                            ),
                            fade_task,
                        ]),
                        None,
                    );
                }
            }
            Message::MarkAsRead => {
                return (Task::none(), Some(Event::MarkAsRead));
            }
            Message::ContentResized(size) => {
                self.content_size = size;
            }
            Message::PendingScrollTo => {
                if let Some(key) = &self.pending_scroll_to {
                    let scroll_to = keyed::find(self.scrollable.clone(), *key)
                        .map(Message::ScrollTo);

                    self.pending_scroll_to = None;
                    self.is_scrolling_to = true;

                    return (scroll_to, None);
                }
            }
            Message::FadeHighlight(hash, generation) => {
                if let Some((current_hash, alpha)) =
                    &mut self.highlighted_message
                    && *current_hash == hash
                    && generation == self.highlight_generation
                {
                    *alpha -= HIGHLIGHT_ALPHA_STEP;
                    if *alpha <= 0.0 {
                        self.highlighted_message = None;
                    } else {
                        return (
                            Task::perform(
                                time::sleep(Duration::from_millis(
                                    HIGHLIGHT_ALPHA_TICK_MS,
                                )),
                                move |()| {
                                    Message::FadeHighlight(hash, generation)
                                },
                            ),
                            None,
                        );
                    }
                }
            }
            Message::HeightsCollected(heights) => {
                for (key, height) in &heights {
                    self.height_cache.insert(*key, *height);
                }

                if let Some(key) = &self.pending_scroll_to {
                    let scroll_to = keyed::find(self.scrollable.clone(), *key)
                        .map(Message::ScrollTo);

                    self.pending_scroll_to = None;
                    self.is_scrolling_to = true;

                    return (scroll_to, None);
                }
            }
        }
        (Task::none(), None)
    }

    pub fn update_pane_size(&mut self, pane_size: Size, config: &Config) {
        let step_messages = step_messages(pane_size.height, config);

        match self.limit {
            Limit::Top(x) if x < step_messages => {
                self.limit = Limit::Top(step_messages);
            }
            Limit::Bottom(x) if x < step_messages => {
                self.limit = Limit::Bottom(step_messages);
            }
            Limit::Around(x, hash) if x < step_messages => {
                self.limit = Limit::Around(step_messages, hash);
            }
            _ => {}
        }

        let width_changed = self.pane_size.width != pane_size.width;

        self.pane_size = pane_size;

        if width_changed {
            self.height_cache.clear();
        }
    }

    pub fn scroll_up_page(&mut self) -> Task<Message> {
        correct_viewport::scroll_by(
            self.scrollable.clone(),
            self.status.anchor(),
            |bounds| scrollable::AbsoluteOffset {
                x: 0.0,
                y: -(bounds.height - 20.0).max(0.0).min(bounds.height),
            },
        )
    }

    pub fn scroll_down_page(&mut self) -> Task<Message> {
        correct_viewport::scroll_by(
            self.scrollable.clone(),
            self.status.anchor(),
            |bounds| scrollable::AbsoluteOffset {
                x: 0.0,
                y: (bounds.height - 20.0).max(0.0).min(bounds.height),
            },
        )
    }

    pub fn scroll_to_start(&mut self, config: &Config) -> Task<Message> {
        self.status = Status::Unlocked;
        self.limit =
            Limit::Top(step_messages(2.0 * self.pane_size.height, config));
        correct_viewport::scroll_to(
            self.scrollable.clone(),
            scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
        )
    }

    pub fn scroll_to_end(&mut self, config: &Config) -> Task<Message> {
        self.status = Status::Bottom;
        self.limit =
            Limit::Bottom(step_messages(2.0 * self.pane_size.height, config));
        correct_viewport::scroll_to(
            self.scrollable.clone(),
            scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
        )
    }

    pub fn is_scrolled_to_bottom(&self) -> bool {
        matches!(self.status, Status::Bottom)
    }

    pub fn scroll_to_message(
        &mut self,
        message: message::Hash,
        kind: Kind,
        history: &history::Manager,
        config: &Config,
    ) -> Task<Message> {
        let Some(history::View {
            old_messages,
            new_messages,
            ..
        }) = history.get_messages(&kind.into(), None)
        else {
            // We're still loading history, which will trigger scroll_to_backlog
            // after loading. If this is set, we will scroll_to_message
            self.pending_scroll_to = Some(keyed::Key::Message(message));

            return Task::none();
        };

        let Some(target) = old_messages
            .iter()
            .chain(&new_messages)
            .find(|m| m.hash == message)
        else {
            return Task::none();
        };

        // Load a window of messages centered on the target.
        let around_count = step_messages(4.0 * self.pane_size.height, config);
        self.limit = Limit::Around(around_count, target.hash);

        self.pending_scroll_to = Some(keyed::Key::Message(message));

        Task::perform(time::sleep(SCROLL_TO_TIMEOUT), move |()| {
            Message::PendingScrollTo
        })
    }

    pub fn scroll_to_backlog(
        &mut self,
        kind: Kind,
        history: &history::Manager,
        config: &Config,
    ) -> Task<Message> {
        if self.pending_scroll_to.is_some() {
            return Task::perform(time::sleep(SCROLL_TO_TIMEOUT), move |()| {
                Message::PendingScrollTo
            });
        }

        let Some(history::View {
            old_messages,
            new_messages,
            ..
        }) = history.get_messages(&kind.into(), None)
        else {
            return Task::none();
        };

        if new_messages.is_empty() {
            return self.scroll_to_end(config);
        }

        // Use the message at the divider boundary as anchor
        let Some(target) = old_messages
            .iter()
            .chain(&new_messages)
            .nth(old_messages.len().saturating_sub(1))
        else {
            return Task::none();
        };

        let around_count = step_messages(4.0 * self.pane_size.height, config);
        self.limit = Limit::Around(around_count, target.hash);

        self.pending_scroll_to = Some(keyed::Key::Divider);

        Task::perform(time::sleep(SCROLL_TO_TIMEOUT), move |()| {
            Message::PendingScrollTo
        })
    }

    pub fn has_pending_scroll_to(&self) -> bool {
        self.pending_scroll_to.is_some()
    }

    pub fn prepare_for_pending_scroll_to(
        &mut self,
        kind: Kind,
        history: &history::Manager,
        config: &Config,
    ) -> Task<Message> {
        let Some(key) = self.pending_scroll_to else {
            return Task::none();
        };

        let Some(history::View {
            old_messages,
            new_messages,
            ..
        }) = history.get_messages(&kind.into(), None)
        else {
            return Task::none();
        };

        let around_count = step_messages(4.0 * self.pane_size.height, config);

        match key {
            keyed::Key::Message(message) | keyed::Key::Preview(message, _) => {
                let Some(target) = old_messages
                    .iter()
                    .chain(&new_messages)
                    .find(|m| m.hash == message)
                else {
                    return Task::none();
                };

                // Load a window of messages centered on the target
                self.limit = Limit::Around(around_count, target.hash);
            }
            keyed::Key::Divider => {
                let Some(target) = old_messages
                    .iter()
                    .chain(&new_messages)
                    .nth(old_messages.len().saturating_sub(1))
                else {
                    return Task::none();
                };

                self.limit = Limit::Around(around_count, target.hash);
            }
        };

        keyed::collect_heights(self.scrollable.clone())
            .map(Message::HeightsCollected)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum Status {
    #[default]
    Bottom,
    Unlocked,
}

impl Status {
    fn anchor(self) -> scrollable::Anchor {
        match self {
            Status::Bottom => scrollable::Anchor::End,
            Status::Unlocked => scrollable::Anchor::Start,
        }
    }

    fn is_top(self, relative_offset: f32) -> bool {
        match self.anchor() {
            scrollable::Anchor::Start => relative_offset == 0.0,
            scrollable::Anchor::End => relative_offset == 1.0,
        }
    }

    fn is_bottom(self, relative_offset: f32) -> bool {
        match self.anchor() {
            scrollable::Anchor::Start => relative_offset == 1.0,
            scrollable::Anchor::End => relative_offset == 0.0,
        }
    }

    fn is_page_from_top(
        self,
        absolute_offset: f32,
        page_height: f32,
        content_height: f32,
    ) -> bool {
        match self.anchor() {
            scrollable::Anchor::Start => absolute_offset <= page_height,
            scrollable::Anchor::End => {
                absolute_offset >= content_height - 2.0 * page_height
            }
        }
    }

    fn is_page_from_bottom(
        self,
        absolute_offset: f32,
        page_height: f32,
        content_height: f32,
    ) -> bool {
        match self.anchor() {
            scrollable::Anchor::Start => {
                absolute_offset >= content_height - 2.0 * page_height
            }
            scrollable::Anchor::End => absolute_offset <= page_height,
        }
    }

    fn flipped(
        self,
        other: Self,
        viewport: scrollable::Viewport,
    ) -> Option<scrollable::AbsoluteOffset> {
        if self.anchor() != other.anchor() {
            let offset = viewport.absolute_offset();
            let reversed_offset = viewport.absolute_offset_reversed();

            Some(scrollable::AbsoluteOffset {
                x: offset.x,
                y: reversed_offset.y,
            })
        } else {
            None
        }
    }
}

fn step_messages(height: f32, config: &Config) -> usize {
    let line_height = theme::resolve_line_height(&config.font);

    (height / line_height).max(8.0) as usize
}

fn timestamp_width(message: &data::Message, config: &Config) -> Option<f32> {
    config
        .buffer
        .format_timestamp(&message.server_time)
        .map(|timestamp| font::width_from_str(&timestamp, &config.font) + 1.0)
}

pub mod keyed {
    use data::message;
    use iced::advanced::widget::{self, Operation};
    use iced::widget::scrollable::{self, AbsoluteOffset};
    use iced::{Rectangle, Task, Vector, advanced};

    use crate::widget::{Element, Renderer, decorate};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Key {
        Divider,
        Message(message::Hash),
        Preview(message::Hash, usize),
    }

    impl Key {
        pub fn message(message: &data::Message) -> Self {
            Self::Message(message.hash)
        }
    }

    pub fn keyed<'a, Message: 'a>(
        key: Key,
        inner: impl Into<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        decorate(inner)
            .operate(
                move |_state: &mut (),
                      inner: &mut Element<'a, Message>,
                      tree: &mut advanced::widget::Tree,
                      layout: advanced::Layout<'_>,
                      renderer: &Renderer,
                      operation: &mut dyn advanced::widget::Operation<()>| {
                    let mut key = key;
                    operation.custom(None, layout.bounds(), &mut key);
                    inner.as_widget_mut().operate(tree, layout, renderer, operation);
                },
            )
            .into()
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Hit {
        pub key: Key,
        pub hit_bounds: Rectangle,
        pub scrollable: Scrollable,
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Scrollable {
        pub viewport: Rectangle,
        pub content: Rectangle,
        pub offset: AbsoluteOffset,
    }

    impl Scrollable {
        pub fn max_vertical_offset(&self) -> f32 {
            (self.content.height - self.viewport.height).max(0.0)
        }

        pub fn reversed_offset(&self) -> AbsoluteOffset {
            AbsoluteOffset {
                x: (self.content.width - self.viewport.width).max(0.0)
                    - self.offset.x,
                y: (self.content.height - self.viewport.height).max(0.0)
                    - self.offset.y,
            }
        }
    }

    impl From<scrollable::Viewport> for Scrollable {
        fn from(viewport: scrollable::Viewport) -> Self {
            Self {
                viewport: viewport.bounds(),
                content: viewport.content_bounds(),
                offset: viewport.absolute_offset(),
            }
        }
    }

    pub fn find(scrollable: widget::Id, key: Key) -> Task<Hit> {
        widget::operate(Find {
            active: false,
            scrollable_id: scrollable,
            key,
            scrollable: None,
            hit_bounds: None,
        })
    }

    #[derive(Debug, Clone)]
    pub struct Find {
        pub active: bool,
        pub key: Key,
        pub scrollable_id: widget::Id,
        pub scrollable: Option<Scrollable>,
        pub hit_bounds: Option<Rectangle>,
    }

    impl Operation<Hit> for Find {
        fn scrollable(
            &mut self,
            id: Option<&widget::Id>,
            bounds: Rectangle,
            content_bounds: Rectangle,
            translation: Vector,
            _state: &mut dyn widget::operation::Scrollable,
        ) {
            if id.is_some_and(|id| *id == self.scrollable_id) {
                self.scrollable = Some(Scrollable {
                    viewport: bounds,
                    content: content_bounds,
                    offset: AbsoluteOffset {
                        x: translation.x,
                        y: translation.y,
                    },
                });
                self.active = true;
            } else {
                self.active = false;
            }
        }

        fn container(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle) {}

        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn Operation<Hit>),
        ) {
            operate(self);
        }

        fn custom(
            &mut self,
            _id: Option<&widget::Id>,
            bounds: Rectangle,
            state: &mut dyn std::any::Any,
        ) {
            if self.active
                && let Some(key) = state.downcast_ref::<Key>()
                && self.key == *key
            {
                self.hit_bounds = Some(bounds);
            }
        }

        fn finish(&self) -> widget::operation::Outcome<Hit> {
            match self.scrollable.zip(self.hit_bounds).map(
                |(scrollable, hit_bounds)| Hit {
                    key: self.key,
                    scrollable,
                    hit_bounds,
                },
            ) {
                Some(hit) => widget::operation::Outcome::Some(hit),
                None => widget::operation::Outcome::None,
            }
        }
    }

    #[derive(Debug, Clone)]
    pub struct TopOfViewport {
        pub active: bool,
        pub scrollable_id: widget::Id,
        pub scrollable: Option<Scrollable>,
        pub hit_bounds: Option<(Key, Rectangle)>,
    }

    impl Operation<Hit> for TopOfViewport {
        fn scrollable(
            &mut self,
            id: Option<&widget::Id>,
            bounds: Rectangle,
            content_bounds: Rectangle,
            translation: Vector,
            _state: &mut dyn widget::operation::Scrollable,
        ) {
            if id.is_some_and(|id| *id == self.scrollable_id) {
                self.scrollable = Some(Scrollable {
                    viewport: bounds,
                    content: content_bounds,
                    offset: AbsoluteOffset {
                        x: translation.x,
                        y: translation.y,
                    },
                });
                self.active = true;
            } else {
                self.active = false;
            }
        }

        fn container(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle) {}

        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn Operation<Hit>),
        ) {
            operate(self);
        }

        fn custom(
            &mut self,
            _id: Option<&widget::Id>,
            bounds: Rectangle,
            state: &mut dyn std::any::Any,
        ) {
            if self.active
                && let Some(key) = state.downcast_ref::<Key>()
                && self.hit_bounds.is_none()
                && self.scrollable.is_some_and(|scrollable| {
                    scrollable.viewport.intersects(
                        &(bounds
                            - Vector::new(
                                scrollable.offset.x,
                                scrollable.offset.y,
                            )),
                    )
                })
            {
                self.hit_bounds = Some((*key, bounds));
            }
        }

        fn finish(&self) -> widget::operation::Outcome<Hit> {
            match self.scrollable.zip(self.hit_bounds).map(
                |(scrollable, (key, hit_bounds))| Hit {
                    key,
                    scrollable,
                    hit_bounds,
                },
            ) {
                Some(hit) => widget::operation::Outcome::Some(hit),
                None => widget::operation::Outcome::None,
            }
        }
    }

    pub struct CollectHeights {
        active: bool,
        scrollable_id: widget::Id,
        heights: Vec<(Key, f32)>,
    }

    impl Operation<Vec<(Key, f32)>> for CollectHeights {
        fn scrollable(
            &mut self,
            id: Option<&widget::Id>,
            _bounds: Rectangle,
            _content_bounds: Rectangle,
            _translation: Vector,
            _state: &mut dyn widget::operation::Scrollable,
        ) {
            self.active = id == Some(&self.scrollable_id);
        }

        fn container(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle) {}

        fn traverse(
            &mut self,
            operate: &mut dyn FnMut(&mut dyn Operation<Vec<(Key, f32)>>),
        ) {
            operate(self);
        }

        fn custom(
            &mut self,
            _id: Option<&widget::Id>,
            bounds: Rectangle,
            state: &mut dyn std::any::Any,
        ) {
            if self.active
                && let Some(key) = state.downcast_ref::<Key>()
                && matches!(key, Key::Message(_) | Key::Divider)
            {
                self.heights.push((*key, bounds.height));
            }
        }

        fn finish(&self) -> widget::operation::Outcome<Vec<(Key, f32)>> {
            if self.heights.is_empty() {
                widget::operation::Outcome::None
            } else {
                widget::operation::Outcome::Some(self.heights.clone())
            }
        }
    }

    pub fn collect_heights(scrollable: widget::Id) -> Task<Vec<(Key, f32)>> {
        widget::operate(CollectHeights {
            active: false,
            scrollable_id: scrollable,
            heights: vec![],
        })
    }
}

mod correct_viewport {
    use std::any::Any;
    use std::sync::{Arc, Mutex};

    use iced::advanced::widget::operation::{Scrollable, scrollable};
    use iced::advanced::widget::{Id, Operation};
    use iced::advanced::{self, widget};
    use iced::widget::scrollable::{AbsoluteOffset, Anchor};
    use iced::{Rectangle, Task, Vector};

    use super::{Message, keyed};
    use crate::widget::{Element, Renderer, decorate};

    pub fn correct_viewport<'a>(
        inner: impl Into<Element<'a, Message>>,
        scrollable: iced::widget::Id,
        enabled: bool,
    ) -> Element<'a, Message> {
        decorate(inner)
            .update({
                let scrollable = scrollable.clone();
                move |state: &mut Option<keyed::Hit>,
                      inner: &mut Element<'a, Message>,
                      tree: &mut advanced::widget::Tree,
                      event: &iced::Event,
                      layout: advanced::Layout<'_>,
                      cursor: advanced::mouse::Cursor,
                      renderer: &Renderer,
                      shell: &mut advanced::Shell<'_, Message>,
                      viewport: &iced::Rectangle| {
                    let is_redraw = matches!(
                        event,
                        iced::Event::Window(iced::window::Event::RedrawRequested(_))
                    );

                    // Check if top-of-viewport element has shifted since we
                    // last scrolled and adjust
                    if let (true, true, Some(old)) = (enabled, is_redraw, &state) {
                        let hit = Arc::new(Mutex::new(None));

                        let mut operation = widget::operation::map(
                            keyed::Find {
                                active: false,
                                key: old.key,
                                scrollable_id: scrollable.clone(),
                                scrollable: None,
                                hit_bounds: None,
                            },
                            {
                                let hit = hit.clone();
                                move |result| {
                                    *hit.lock().unwrap() = Some(result);
                                }
                            },
                        );

                        inner
                            .as_widget_mut()
                            .operate(tree, layout, renderer, &mut operation);
                        operation.finish();
                        drop(operation);

                        if let Some(new) = Arc::into_inner(hit)
                            .and_then(|m| m.into_inner().ok())
                            .flatten()
                        {
                            // Something shifted this, let's put it back to the
                            // top of the viewport
                            if new.hit_bounds.y != old.hit_bounds.y {
                                let viewport_offset = old.scrollable.viewport.y
                                    - (old.hit_bounds.y - old.scrollable.offset.y);

                                // New offset needed to place same element back to same offset
                                // from top of viewport
                                let new_offset = f32::min(
                                    (new.hit_bounds.y + viewport_offset)
                                        - new.scrollable.viewport.y,
                                    new.scrollable.content.height - new.scrollable.viewport.height,
                                );

                                let mut operation = scrollable::scroll_to(
                                    scrollable.clone(),
                                    scrollable::AbsoluteOffset {
                                        x: None,
                                        y: Some(new_offset),
                                    },
                                );
                                inner
                                    .as_widget_mut()
                                    .operate(tree, layout, renderer, &mut operation);
                                operation.finish();
                            }
                        }
                    }

                    let mut messages = vec![];
                    let mut local_shell = shell.local(&mut messages);

                    inner.as_widget_mut().update(
                        tree,
                        event,
                        layout,
                        cursor,
                        renderer,
                        &mut local_shell,
                        viewport,
                    );

                    // Merge shell (we can't use Shell::merge as we'd lose
                    // access to messages)
                    {
                        match local_shell.redraw_request() {
                            iced::window::RedrawRequest::NextFrame => shell.request_redraw(),
                            iced::window::RedrawRequest::At(at) => shell.request_redraw_at(at),
                            iced::window::RedrawRequest::Wait => {}
                        }

                        if let Some(diff) = shell.is_layout_invalid() {
                            shell.invalidate_layout_with(diff);
                        }

                        if local_shell.are_widgets_invalid() {
                            shell.invalidate_widgets();
                        }

                        if local_shell.is_event_captured() {
                            shell.capture_event();
                        }
                    }

                    let mut is_scrolled = false;
                    for message in messages {
                        is_scrolled |=
                            matches!(message, Message::Scrolled { .. });
                        shell.publish(message);
                    }

                    // Re-query top of viewport any-time we scroll
                    if is_scrolled {
                        let hit = Arc::new(Mutex::new(None));

                        let mut operation = widget::operation::map(
                            keyed::TopOfViewport {
                                active: false,
                                scrollable_id: scrollable.clone(),
                                scrollable: None,
                                hit_bounds: None,
                            },
                            {
                                let hit = hit.clone();
                                move |result| {
                                    *hit.lock().unwrap() = Some(result);
                                }
                            },
                        );

                        inner
                            .as_widget_mut()
                            .operate(tree, layout, renderer, &mut operation);
                        operation.finish();
                        drop(operation);

                        *state = Arc::into_inner(hit)
                            .and_then(|m| m.into_inner().ok())
                            .flatten();
                    }
                }
            })
            .operate(
                move |state: &mut Option<keyed::Hit>,
                      inner: &mut Element<'a, Message>,
                      tree: &mut advanced::widget::Tree,
                      layout: advanced::Layout<'_>,
                      renderer: &Renderer,
                      operation: &mut dyn advanced::widget::Operation<()>| {
                    inner.as_widget_mut().operate(tree, layout, renderer, operation);

                    let mut is_scroll_to = false;

                    operation.custom(
                        Some(&scrollable),
                        layout.bounds(),
                        &mut is_scroll_to,
                    );

                    if is_scroll_to {
                        let hit = Arc::new(Mutex::new(None));

                        let mut operation = widget::operation::map(
                            keyed::TopOfViewport {
                                active: false,
                                scrollable_id: scrollable.clone(),
                                scrollable: None,
                                hit_bounds: None,
                            },
                            {
                                let hit = hit.clone();
                                move |result| {
                                    *hit.lock().unwrap() = Some(result);
                                }
                            },
                        );

                        inner
                            .as_widget_mut()
                            .operate(tree, layout, renderer, &mut operation);
                        operation.finish();
                        drop(operation);

                        *state = Arc::into_inner(hit)
                            .and_then(|m| m.into_inner().ok())
                            .flatten();
                    }
                },
            )
            .into()
    }

    pub fn scroll_to<T: Send + 'static>(
        target: impl Into<Id>,
        offset: AbsoluteOffset,
    ) -> Task<T> {
        struct ScrollTo {
            target: Id,
            offset: AbsoluteOffset,
        }

        impl<T> Operation<T> for ScrollTo {
            fn container(&mut self, _id: Option<&Id>, _bounds: Rectangle) {}

            fn traverse(
                &mut self,
                operate: &mut dyn FnMut(&mut dyn Operation<T>),
            ) {
                operate(self);
            }

            fn scrollable(
                &mut self,
                id: Option<&Id>,
                _bounds: Rectangle,
                _content_bounds: Rectangle,
                _translation: Vector,
                state: &mut dyn Scrollable,
            ) {
                if id.is_some_and(|id| *id == self.target) {
                    state.scroll_to(self.offset.into());
                }
            }

            fn custom(
                &mut self,
                id: Option<&Id>,
                _bounds: Rectangle,
                state: &mut dyn Any,
            ) {
                if id.is_some_and(|id| *id == self.target)
                    && let Some(is_scroll_to) = state.downcast_mut::<bool>()
                {
                    *is_scroll_to = true;
                }
            }
        }

        widget::operate(ScrollTo {
            target: target.into(),
            offset,
        })
    }

    pub fn scroll_by<T: Send + 'static>(
        target: impl Into<Id>,
        anchor: Anchor,
        f: impl Fn(Rectangle) -> AbsoluteOffset + Send + 'static,
    ) -> Task<T> {
        struct ScrollBy {
            target: Id,
            anchor: Anchor,
            f: Box<dyn Fn(Rectangle) -> AbsoluteOffset + Send>,
        }

        impl<T> Operation<T> for ScrollBy {
            fn container(&mut self, _id: Option<&Id>, _bounds: Rectangle) {}

            fn traverse(
                &mut self,
                operate: &mut dyn FnMut(&mut dyn Operation<T>),
            ) {
                operate(self);
            }

            fn scrollable(
                &mut self,
                id: Option<&Id>,
                bounds: Rectangle,
                content_bounds: Rectangle,
                _translation: Vector,
                state: &mut dyn Scrollable,
            ) {
                if Some(&self.target) == id {
                    let mut offset = (self.f)(bounds);

                    // Flip offset
                    if matches!(self.anchor, Anchor::End) {
                        offset.y = -offset.y;
                    }

                    state.scroll_by(offset, bounds, content_bounds);
                }
            }

            fn custom(
                &mut self,
                id: Option<&Id>,
                _bounds: Rectangle,
                state: &mut dyn Any,
            ) {
                if id.is_some_and(|id| *id == self.target)
                    && let Some(is_scroll_to) = state.downcast_mut::<bool>()
                {
                    *is_scroll_to = true;
                }
            }
        }

        widget::operate(ScrollBy {
            target: target.into(),
            anchor,
            f: Box::new(f),
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at_local(date: NaiveDate, hour: u32, minute: u32) -> i64 {
        Local
            .from_local_datetime(
                &date
                    .and_hms_opt(hour, minute, 0)
                    .expect("valid wall clock time"),
            )
            .earliest()
            .expect("wall clock time exists locally")
            .timestamp_millis()
    }

    fn from_peer(timestamp_ms: i64) -> data::Message {
        data::Message::received(
            ConvoId::from("c1"),
            Some(data::Address::from("deadbeef")),
            "hello".to_string(),
            timestamp_ms,
        )
    }

    fn from_other_peer(timestamp_ms: i64) -> data::Message {
        data::Message::received(
            ConvoId::from("c1"),
            Some(data::Address::from("cafebabe")),
            "hello".to_string(),
            timestamp_ms,
        )
    }

    /// QML `SameSenderAsPreviousRole` compares the calendar day too, so the
    /// message sitting under a day chip always carries its sender's name.
    #[test]
    fn sender_run_breaks_at_local_midnight() {
        let monday = NaiveDate::from_ymd_opt(2024, 6, 17).expect("valid date");
        let tuesday = NaiveDate::from_ymd_opt(2024, 6, 18).expect("valid date");

        let before = from_peer(at_local(monday, 23, 50));
        let last_of_day = from_peer(at_local(monday, 23, 58));
        let first_of_day = from_peer(at_local(tuesday, 0, 3));

        assert!(continues_sender_run(&last_of_day, Some(&before), None));

        assert!(
            is_consecutive_message(&first_of_day, Some(&last_of_day), None),
            "same sender, minutes apart: consecutive for timestamp hiding"
        );
        assert!(
            !continues_sender_run(&first_of_day, Some(&last_of_day), None),
            "a new local day heads a new run"
        );

        for hide_consecutive in [
            HideConsecutiveEnabled::Disabled,
            HideConsecutiveEnabled::Enabled(None),
            HideConsecutiveEnabled::Enabled(chrono::TimeDelta::try_hours(1)),
        ] {
            assert!(!hide_sender(
                &first_of_day,
                Some(&last_of_day),
                hide_consecutive,
                true
            ));
        }
    }

    /// Alignment is deliberately not a factor: a thread collapses runs
    /// whether the sender sits beside the message or above it, and turning
    /// `hide_consecutive` on never restores a label it was hiding while off.
    #[test]
    fn hide_sender_collapses_runs_in_both_branches() {
        let day = NaiveDate::from_ymd_opt(2024, 6, 17).expect("valid date");
        let first = from_peer(at_local(day, 10, 0));
        let second = from_peer(at_local(day, 10, 1));
        let stranger = from_other_peer(at_local(day, 10, 2));

        assert!(hide_sender(
            &second,
            Some(&first),
            HideConsecutiveEnabled::Disabled,
            true
        ));
        assert!(hide_sender(
            &second,
            Some(&first),
            HideConsecutiveEnabled::Enabled(None),
            true
        ));

        // A layout that names every sender (the log) only collapses on request
        assert!(!hide_sender(
            &second,
            Some(&first),
            HideConsecutiveEnabled::Disabled,
            false
        ));
        assert!(hide_sender(
            &second,
            Some(&first),
            HideConsecutiveEnabled::Enabled(None),
            false
        ));

        // Someone else's message always heads its own run
        assert!(!hide_sender(
            &stranger,
            Some(&second),
            HideConsecutiveEnabled::Disabled,
            true
        ));
        // ... and so does the first message in the thread
        assert!(!hide_sender(
            &first,
            None,
            HideConsecutiveEnabled::Disabled,
            true
        ));
    }

    #[test]
    fn hide_sender_honors_the_configured_window() {
        let day = NaiveDate::from_ymd_opt(2024, 6, 17).expect("valid date");
        let first = from_peer(at_local(day, 10, 0));
        let much_later = from_peer(at_local(day, 14, 0));

        assert!(hide_sender(
            &much_later,
            Some(&first),
            HideConsecutiveEnabled::Enabled(chrono::TimeDelta::try_hours(5)),
            true
        ));
        assert!(!hide_sender(
            &much_later,
            Some(&first),
            HideConsecutiveEnabled::Enabled(chrono::TimeDelta::try_minutes(5)),
            true
        ));
    }
}
