use std::time::Duration;

use data::address::Address;
use data::conversation::{ConvoId, Kind};
use data::{Config, history};
use iced::widget::{Space, column, container, row, text};
use iced::{Length, Size, Task, alignment, padding};
use tokio::time::{self, Instant};

use super::message_view::ConversationLayout;
use super::{context_menu, input_view, scroll_view};
use crate::widget::{Element, skeleton};
use crate::{Theme, font, theme};

pub mod details;
pub mod roster;

/// How long a thread stays blank before it shows placeholders, so a
/// switch that resolves quickly never flashes one (QML `graceTimer`).
const LOAD_GRACE: Duration = Duration::from_millis(150);

/// How long after a thread's messages land before an empty one says so,
/// so rows a beat behind are never called absent (QML `settleTimer`).
const LOAD_SETTLE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone)]
pub enum Message {
    ScrollView(scroll_view::Message),
    InputView(input_view::Message),
    ContextMenu(context_menu::Message),
    Roster(roster::Message),
    Details(details::Message),
    /// A grace or settle window closed. The windows are wall-clock, so
    /// this only has to bring the thread back for a redraw.
    LoadWindowElapsed,
}

pub enum Event {
    ContextMenu(context_menu::Event),
    MarkAsRead(history::Kind),
    OpenUrl(String),
    SendMessage {
        convo_id: ConvoId,
        content: String,
    },
    Command(data::Command),
    /// Copy-to-clipboard requests from the roster and details panels.
    CopyText(String),
    /// Open (or create) a direct conversation with the address.
    OpenDm(String),
    /// Open the add-member dialog for this group.
    OpenAddMember(ConvoId),
}

pub fn view<'a>(
    state: &'a Conversation,
    conversation: Option<&'a data::Conversation>,
    our_address: Option<&'a Address>,
    can_act: bool,
    history: &'a history::Manager,
    settings: Option<&'a data::buffer::Settings>,
    config: &'a Config,
    theme: &'a Theme,
    is_focused: bool,
) -> Element<'a, Message> {
    let is_group = conversation
        .is_some_and(|conversation| conversation.kind == Kind::Group);

    let header = conversation.map(|conversation| {
        let title = text(conversation.display_name())
            .style(theme::text::primary)
            .font_maybe(theme::font_style::primary(theme).map(font::get));

        let member_count =
            (is_group && !conversation.members.is_empty()).then(|| {
                text(format!("{} members", conversation.joined_member_count()))
                    .style(theme::text::secondary)
                    .font_maybe(
                        theme::font_style::secondary(theme).map(font::get),
                    )
            });

        container(
            row![title, member_count]
                .spacing(8)
                .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .padding([4, 8])
        .style(theme::container::buffer_title_bar)
    });

    let layout = ConversationLayout {
        config,
        theme,
        our_address,
        is_group,
    };

    let kind = history::Kind::Conversation(state.convo_id.clone());

    // The module is the message store: before `MessagesLoaded` folds the
    // fetch in, the history is `Partial`. A wait shorter than the grace
    // window shows nothing at all rather than flashing placeholders, and
    // a thread is not called empty until its rows have had the settle
    // window to land.
    let messages: Element<'a, Message> = match history.get_messages(&kind, None)
    {
        None if state.opened_at.elapsed() < LOAD_GRACE => Space::new().into(),
        None => skeleton::messages(),
        Some(view) if view.total == 0 => {
            if state.settled() {
                empty_thread(theme)
            } else {
                Space::new().into()
            }
        }
        Some(_) => scroll_view::view(
            &state.scroll_view,
            scroll_view::Kind::Conversation(&state.convo_id),
            history,
            config,
            theme,
            layout,
        )
        .map(Message::ScrollView),
    };

    let messages = container(messages).width(Length::Fill).height(Length::Fill);

    let show_text_input = match config.buffer.text_input.visibility {
        data::config::buffer::text_input::Visibility::Focused => is_focused,
        data::config::buffer::text_input::Visibility::Always => true,
    };

    let text_input = show_text_input.then(|| {
        input_view::view(&state.input_view, can_act, config, theme)
            .map(Message::InputView)
    });

    let thread = column![header, messages, text_input].height(Length::Fill);

    let member_list_enabled = is_group
        && settings.map_or(
            config.buffer.conversation.member_list.enabled,
            |settings| settings.conversation.member_list.enabled,
        );

    let side_panels = conversation
        .filter(|_| member_list_enabled || state.show_details)
        .map(|conversation| {
            let details = state.show_details.then(|| {
                details::view(conversation, state.details, theme)
                    .map(Message::Details)
            });

            let member_list = member_list_enabled.then(|| {
                roster::view(conversation, can_act, config, theme)
                    .map(Message::Roster)
            });

            // The side panels are a fixed column beside the thread (QML
            // `Layout.fillWidth: false`); without a width of its own the
            // column reads as fluid and takes half the pane. The roster
            // keeps its configured width, widened to the details panel's
            // where that is open so a key/value row fits on one line.
            let roster_width = config
                .buffer
                .conversation
                .member_list
                .width
                .unwrap_or(roster::DEFAULT_WIDTH);

            column![details, member_list]
                .spacing(8)
                .width(Length::Fixed(if state.show_details {
                    roster_width.max(details::WIDTH)
                } else {
                    roster_width
                }))
                .height(Length::Fill)
        });

    let content = match config.buffer.conversation.member_list.position {
        data::buffer::conversation::Position::Left => {
            row![side_panels, thread]
        }
        data::buffer::conversation::Position::Right => {
            row![thread, side_panels]
        }
    }
    .height(Length::Fill)
    .padding(padding::left(8).right(8));

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// What a thread with nothing in it says (QML `threadEmptyState`).
fn empty_thread<'a>(theme: &'a Theme) -> Element<'a, Message> {
    container(
        text("No messages yet")
            .style(theme::text::secondary)
            .font_maybe(theme::font_style::secondary(theme).map(font::get)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(alignment::Horizontal::Center)
    .align_y(alignment::Vertical::Center)
    .into()
}

#[derive(Debug, Clone)]
pub struct Conversation {
    pub convo_id: ConvoId,
    pub scroll_view: scroll_view::State,
    pub input_view: input_view::State,
    /// Whether the details panel is showing; runtime-only, left as the
    /// user last set it (QML `detailsShown` parity).
    pub show_details: bool,
    /// Copy-flash state for the details panel's copyable rows.
    pub details: details::State,
    /// When this thread was opened, for the grace window. A
    /// `tokio::time::Instant` so the window and the redraw timer below sit
    /// on the same clock: outside a runtime it is `std::time::Instant`,
    /// under `#[tokio::test(start_paused = true)]` it is the virtual one.
    opened_at: Instant,
    /// When its messages landed, for the settle window; `None` while the
    /// module still owes them.
    loaded_at: Option<Instant>,
}

impl Conversation {
    pub fn new(
        convo_id: ConvoId,
        history: &history::Manager,
        pane_size: Size,
        config: &Config,
    ) -> Self {
        // A thread whose messages are already in hand waits for nothing.
        let loaded = history
            .get_messages(&history::Kind::Conversation(convo_id.clone()), None)
            .is_some();

        Self {
            input_view: input_view::State::new(history.input(&convo_id)),
            convo_id,
            scroll_view: scroll_view::State::new(pane_size, config),
            show_details: false,
            details: details::State::default(),
            opened_at: Instant::now(),
            loaded_at: loaded.then(Instant::now),
        }
    }

    /// Whether an empty thread has waited out its settle window and can
    /// say so.
    fn settled(&self) -> bool {
        self.loaded_at
            .is_some_and(|loaded_at| loaded_at.elapsed() >= LOAD_SETTLE)
    }

    /// The module's messages landed: open the settle window and come back
    /// for a redraw when it closes.
    pub fn messages_loaded(&mut self) -> Task<Message> {
        self.loaded_at = Some(Instant::now());

        redraw_after(LOAD_SETTLE)
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
                    scroll_view::Kind::Conversation(&self.convo_id),
                    history,
                    config,
                );

                let event = event.map(|event| match event {
                    scroll_view::Event::ContextMenu(event) => {
                        Event::ContextMenu(event)
                    }
                    scroll_view::Event::MarkAsRead => Event::MarkAsRead(
                        history::Kind::Conversation(self.convo_id.clone()),
                    ),
                    scroll_view::Event::OpenUrl(url) => Event::OpenUrl(url),
                });

                (command.map(Message::ScrollView), event)
            }
            Message::InputView(message) => {
                let (command, event) = self.input_view.update(
                    message,
                    &self.convo_id,
                    history,
                    config,
                );
                let command = command.map(Message::InputView);

                match event {
                    Some(input_view::Event::SendMessage {
                        convo_id,
                        content,
                    }) => {
                        let command = Task::batch(vec![
                            command,
                            self.scroll_view
                                .scroll_to_end(config)
                                .map(Message::ScrollView),
                        ]);

                        (
                            command,
                            Some(Event::SendMessage { convo_id, content }),
                        )
                    }
                    Some(input_view::Event::Command(command_parsed)) => {
                        (command, Some(Event::Command(command_parsed)))
                    }
                    None => (command, None),
                }
            }
            Message::ContextMenu(message) => (
                Task::none(),
                Some(Event::ContextMenu(context_menu::update(message))),
            ),
            Message::Roster(message) => match message {
                roster::Message::AddMember => (
                    Task::none(),
                    Some(Event::OpenAddMember(self.convo_id.clone())),
                ),
                roster::Message::CopyAddress(address) => {
                    (Task::none(), Some(Event::CopyText(address)))
                }
                roster::Message::Dm(address) => {
                    (Task::none(), Some(Event::OpenDm(address)))
                }
            },
            Message::Details(message) => match message {
                details::Message::Close => {
                    self.show_details = false;
                    (Task::none(), None)
                }
                details::Message::Copy(row, text) => {
                    let generation = self.details.copy_started(row);

                    (
                        Task::perform(
                            time::sleep(details::COPY_FLASH_DURATION),
                            move |()| {
                                Message::Details(
                                    details::Message::CopyFlashExpired(
                                        generation,
                                    ),
                                )
                            },
                        ),
                        Some(Event::CopyText(text)),
                    )
                }
                details::Message::CopyFlashExpired(generation) => {
                    self.details.copy_expired(generation);
                    (Task::none(), None)
                }
            },
            Message::LoadWindowElapsed => (Task::none(), None),
        }
    }

    pub fn focus(&self) -> Task<Message> {
        Task::batch(vec![
            self.input_view.focus().map(Message::InputView),
            // Whichever window this thread opened into, come back when it
            // closes: nothing else redraws on a timer.
            redraw_after(if self.loaded_at.is_some() {
                LOAD_SETTLE
            } else {
                LOAD_GRACE
            }),
        ])
    }

    pub fn reset(&mut self) {
        self.input_view.reset();
    }

    pub fn toggle_details(&mut self) {
        self.show_details = !self.show_details;
    }

    pub fn restore_draft(&mut self, content: &str) {
        self.input_view.restore_draft(content);
    }
}

fn redraw_after(delay: Duration) -> Task<Message> {
    Task::perform(time::sleep(delay), |()| Message::LoadWindowElapsed)
}
