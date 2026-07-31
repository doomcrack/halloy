use std::borrow::Cow;

use data::config::buffer::text_input::KeyBindings;
use data::conversation::ConvoId;
use data::input::{self, RawInput};
use data::{Config, history};
use iced::advanced::widget::Tree;
use iced::advanced::{Layout, Shell, mouse};
use iced::widget::text::{Shaping, Wrapping};
use iced::widget::{self, button, column, container, row, text_editor};
use iced::{Length, Task, clipboard, event, keyboard};

use self::completion::{Arrow, Completion};
use crate::widget::key_press::is_numpad;
use crate::widget::{
    Element, Renderer, Text, anchored_overlay, context_menu, decorate, text,
    text_editor_key_bindings,
};
use crate::{Theme, font, theme};

mod completion;

pub enum Event {
    /// Plain text submitted with Enter; the dashboard forwards it to the
    /// backend as `Control::SendMessage`.
    SendMessage { convo_id: ConvoId, content: String },
    /// A parsed slash command; the dashboard maps it onto backend controls
    /// (dm/group/add/nick) or local actions (details/clear).
    Command(data::Command),
}

#[derive(Debug, Clone)]
pub enum Message {
    Action(text_editor::Action),
    Send,
    Kill(text_editor_key_bindings::Kill, bool),
    Tab(bool),
    Up(bool),
    Down(bool),
    Escape,
    Paste,
    SelectAll,
    CopyAll,
    Copy,
    Cut,
    CompletionSelected(usize),
}

#[derive(Debug, Clone, Copy)]
pub enum Actions {
    Cut,
    Copy,
    CopyAll,
    Paste,
    SelectAll,
}

impl Actions {
    fn list() -> Vec<Self> {
        vec![
            Self::Cut,
            Self::Copy,
            Self::CopyAll,
            Self::Paste,
            Self::SelectAll,
        ]
    }
}

#[derive(Debug, Clone)]
enum Notice {
    Error(String),
}

fn kill_binding(
    kill: text_editor_key_bindings::Kill,
) -> text_editor::Binding<Message> {
    text_editor::Binding::Custom(Message::Kill(kill, true))
}

fn platform_specific_key_bindings(
    key_press: text_editor::KeyPress,
    selection: Option<&str>,
) -> Option<text_editor::Binding<Message>> {
    paste_key_binding(&key_press).or_else(|| {
        text_editor_key_bindings::platform_kill(
            &key_press,
            selection.is_some(),
            |kill| text_editor::Binding::Custom(Message::Kill(kill, false)),
        )
    })
}

#[cfg(target_os = "macos")]
fn paste_key_binding(
    key_press: &text_editor::KeyPress,
) -> Option<text_editor::Binding<Message>> {
    (matches!(key_press.key.as_ref(), iced::keyboard::Key::Character("v"))
        && key_press.modifiers.logo())
    .then_some(text_editor::Binding::Custom(Message::Paste))
}

#[cfg(not(target_os = "macos"))]
fn paste_key_binding(
    key_press: &text_editor::KeyPress,
) -> Option<text_editor::Binding<Message>> {
    match key_press.key.as_ref() {
        iced::keyboard::Key::Named(iced::keyboard::key::Named::Insert)
            if key_press.modifiers.shift() && key_press.text.is_none() =>
        {
            Some(text_editor::Binding::Custom(Message::Paste))
        }
        iced::keyboard::Key::Character("v")
            if key_press.modifiers.control() =>
        {
            Some(text_editor::Binding::Custom(Message::Paste))
        }
        _ => None,
    }
}

pub fn view<'a>(
    state: &'a State,
    can_act: bool,
    config: &'a Config,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let style = if state.notice.is_some() {
        theme::text_editor::error
    } else {
        theme::text_editor::primary
    };

    let placeholder = if can_act {
        "Send message..."
    } else {
        "Waiting for connection..."
    };

    let mut text_input = text_editor(&state.input_content)
        .id(state.input_id.clone())
        .placeholder(placeholder)
        .padding([2, 4])
        .wrapping(Wrapping::WordOrGlyph)
        .height(Length::Shrink)
        .line_height(theme::line_height(&config.font))
        .style(style);

    if can_act {
        text_input = text_input.on_action(Message::Action).key_binding(
            move |key_press| {
                if !matches!(
                    key_press.status,
                    iced::widget::text_editor::Status::Focused { .. }
                ) {
                    return None;
                }

                // Try emacs bindings first if enabled
                if matches!(
                    config.buffer.text_input.key_bindings,
                    KeyBindings::Emacs
                ) && let Some(binding) =
                    text_editor_key_bindings::emacs(&key_press, kill_binding)
                {
                    return Some(binding);
                }

                // Platform specific key bindings
                if let Some(binding) = platform_specific_key_bindings(
                    key_press.clone(),
                    state.input_content.selection().as_deref(),
                ) {
                    return Some(binding);
                }

                // Handling for numpad keys: treat a numpad enter the same as
                // a normal enter; treat numpad keys as character keys when
                // numlock is on (i.e. text.is_some())
                let key = if key_press.physical_key
                    == iced::keyboard::key::Physical::Code(
                        iced::keyboard::key::Code::NumpadEnter,
                    ) {
                    Cow::Owned(iced::keyboard::Key::Named(
                        iced::keyboard::key::Named::Enter,
                    ))
                } else if is_numpad(&key_press.physical_key)
                    && let Some(text) = &key_press.text
                {
                    Cow::Owned(keyboard::Key::Character(text.clone()))
                } else {
                    Cow::Borrowed(&key_press.key)
                };

                match *key {
                    // Send
                    iced::keyboard::Key::Named(
                        iced::keyboard::key::Named::Enter,
                    ) => Some(text_editor::Binding::Custom(Message::Send)),
                    // Tab
                    iced::keyboard::Key::Named(
                        iced::keyboard::key::Named::Tab,
                    ) => Some(text_editor::Binding::Custom(Message::Tab(
                        key_press.modifiers.shift(),
                    ))),
                    // Up
                    iced::keyboard::Key::Named(
                        iced::keyboard::key::Named::ArrowUp,
                    ) => Some(text_editor::Binding::Custom(Message::Up(
                        key_press.modifiers.shift(),
                    ))),
                    // Down
                    iced::keyboard::Key::Named(
                        iced::keyboard::key::Named::ArrowDown,
                    ) => Some(text_editor::Binding::Custom(Message::Down(
                        key_press.modifiers.shift(),
                    ))),
                    // Escape
                    iced::keyboard::Key::Named(
                        iced::keyboard::key::Named::Escape,
                    ) => Some(text_editor::Binding::Custom(Message::Escape)),
                    _ => text_editor::Binding::from_key_press(key_press),
                }
            },
        );
    }

    let text_input = decorate(text_input).update(
        move |_state: &mut State,
              inner: &mut Element<'a, Message>,
              tree: &mut Tree,
              event: &iced::Event,
              layout: Layout<'_>,
              cursor: mouse::Cursor,
              renderer: &Renderer,
              shell: &mut Shell<'_, Message>,
              viewport: &iced::Rectangle| {
            if let event::Event::Mouse(mouse::Event::WheelScrolled { .. }) =
                event
            {
                return;
            };

            inner
                .as_widget_mut()
                .update(tree, event, layout, cursor, renderer, shell, viewport);
        },
    );

    let wrapped_input: Element<'a, Message> = context_menu(
        context_menu::MouseButton::default(),
        context_menu::Anchor::Cursor,
        context_menu::ToggleBehavior::KeepOpen,
        Some(mouse::Interaction::Text),
        text_input,
        Actions::list(),
        move |menu, length| {
            let context_button =
                |title: Text<'a>,
                 keybind: Option<data::shortcut::KeyBind>,
                 message: Option<Message>| {
                    button(
                        row![
                            title.line_height(theme::line_height(&config.font)),
                            keybind.map(|kb| {
                                text(format!("({kb})"))
                                    .shaping(Shaping::Advanced)
                                    .size(theme::TEXT_SIZE - 2.0)
                                    .style(theme::text::secondary)
                                    .font_maybe(
                                        theme::font_style::secondary(theme)
                                            .map(font::get),
                                    )
                            }),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    )
                    .width(length)
                    .padding(config.context_menu.padding.entry)
                    .on_press_maybe(message)
                    .into()
                };

            match menu {
                Actions::Cut => context_button(
                    text("Cut"),
                    Some(data::shortcut::cut()),
                    state.input_content.selection().map(|_| Message::Cut),
                ),
                Actions::Copy => context_button(
                    text("Copy"),
                    Some(data::shortcut::copy()),
                    state.input_content.selection().map(|_| Message::Copy),
                ),
                Actions::CopyAll => context_button(
                    text("Copy All"),
                    None,
                    if !state.input_content.text().is_empty() {
                        Some(Message::CopyAll)
                    } else {
                        None
                    },
                ),
                Actions::SelectAll => context_button(
                    text("Select All"),
                    Some(data::shortcut::select_all()),
                    if !state.input_content.text().is_empty() {
                        Some(Message::SelectAll)
                    } else {
                        None
                    },
                ),
                Actions::Paste => context_button(
                    text("Paste"),
                    Some(data::shortcut::paste()),
                    Some(Message::Paste),
                ),
            }
        },
    )
    .into();

    let input_row = container(
        row![wrapped_input]
            .spacing(4)
            .height(Length::Shrink)
            .align_y(iced::Alignment::Center),
    )
    .padding(8);

    let styled_input =
        container(input_row).style(theme::container::buffer_text_input);

    let notice = state
        .notice
        .as_ref()
        .map(|notice| notice_view(notice, theme));

    let base = column![notice, styled_input]
        .spacing(4)
        .padding(iced::padding::top(4));

    let overlay = state
        .completion
        .view(config, theme, Message::CompletionSelected)
        .unwrap_or_else(|| row![].into());

    anchored_overlay(base, overlay, anchored_overlay::Anchor::AboveTop, 4.0)
}

fn notice_view<'a, Message: 'a>(
    notice: &'a Notice,
    theme: &'a Theme,
) -> Element<'a, Message> {
    container(match notice {
        Notice::Error(notice_string) => text(notice_string)
            .style(theme::text::error)
            .font_maybe(theme::font_style::error(theme).map(font::get)),
    })
    .padding(8)
    .style(theme::container::tooltip)
    .into()
}

#[derive(Debug, Clone)]
pub struct State {
    input_id: widget::Id,
    input_content: text_editor::Content,
    notice: Option<Notice>,
    selected_history: Option<usize>,
    completion: Completion,
}

impl Default for State {
    fn default() -> Self {
        Self {
            input_id: widget::Id::unique(),
            input_content: text_editor::Content::new(),
            notice: None,
            selected_history: None,
            completion: Completion::default(),
        }
    }
}

impl State {
    pub fn new(cache: input::Cache<'_>) -> Self {
        let mut input_content = if cache.draft_message.is_empty() {
            text_editor::Content::new()
        } else {
            text_editor::Content::with_text(cache.draft_message)
        };

        input_content.perform(text_editor::Action::Move(
            text_editor::Motion::DocumentEnd,
        ));

        Self {
            input_content,
            ..Self::default()
        }
    }

    pub fn update(
        &mut self,
        message: Message,
        convo_id: &ConvoId,
        history: &mut history::Manager,
        config: &Config,
    ) -> (Task<Message>, Option<Event>) {
        match message {
            Message::Action(action) => {
                let is_edit = action.is_edit();

                self.input_content.perform(action);

                if is_edit {
                    self.notice = None;
                    self.selected_history = None;

                    let input = self.input_content.text();
                    let (cursor_position, cursor_is_selection) = self.cursor();

                    self.completion.process(
                        &input,
                        cursor_position,
                        cursor_is_selection,
                        config,
                    );

                    if let Some(actions) =
                        self.completion.complete_emoji(&input, cursor_position)
                    {
                        for action in actions {
                            self.input_content.perform(action);
                        }
                    }

                    history.record_draft(RawInput {
                        convo_id: convo_id.clone(),
                        text: self.input_content.text(),
                    });
                }

                (Task::none(), None)
            }
            Message::Send => {
                // Enter with an open picker completes instead of sending.
                if let Some(entry) = self.completion.select(config) {
                    self.apply_completion(&entry, convo_id, history);

                    return (Task::none(), None);
                }

                self.notice = None;
                self.selected_history = None;

                let raw_input = self.input_content.text();

                match input::parse(&raw_input) {
                    Ok(input::Parsed::Text(content)) => {
                        self.completion.reset();
                        history
                            .record_input_history(convo_id, raw_input.clone());
                        self.input_content = text_editor::Content::new();
                        history.record_draft(RawInput {
                            convo_id: convo_id.clone(),
                            text: String::new(),
                        });

                        (
                            Task::none(),
                            Some(Event::SendMessage {
                                convo_id: convo_id.clone(),
                                content,
                            }),
                        )
                    }
                    Ok(input::Parsed::Command(command)) => {
                        self.completion.reset();
                        history
                            .record_input_history(convo_id, raw_input.clone());
                        self.input_content = text_editor::Content::new();
                        history.record_draft(RawInput {
                            convo_id: convo_id.clone(),
                            text: String::new(),
                        });

                        (Task::none(), Some(Event::Command(command)))
                    }
                    Err(input::Error::Empty) => (Task::none(), None),
                    Err(error) => {
                        self.notice = Some(Notice::Error(error.to_string()));
                        (Task::none(), None)
                    }
                }
            }
            Message::Tab(reverse) => {
                if let Some(entry) = self.completion.tab(reverse, config) {
                    self.apply_completion(&entry, convo_id, history);
                }

                (Task::none(), None)
            }
            Message::CompletionSelected(index) => {
                if let Some(entry) = self.completion.select_at(index, config) {
                    self.apply_completion(&entry, convo_id, history);
                }

                (self.focus(), None)
            }
            Message::Up(shift) => {
                if !shift && self.completion.arrow(Arrow::Up) {
                    return (Task::none(), None);
                }

                if shift {
                    self.input_content.perform(text_editor::Action::Select(
                        text_editor::Motion::DocumentStart,
                    ));

                    return (Task::none(), None);
                }

                let cache = history.input(convo_id);

                if !cache.history.is_empty() {
                    if let Some(index) = self.selected_history.as_mut() {
                        if *index == cache.history.len().saturating_sub(1) {
                            self.input_content.perform(
                                text_editor::Action::Move(
                                    text_editor::Motion::DocumentStart,
                                ),
                            );

                            return (Task::none(), None);
                        }

                        *index += 1;
                    } else {
                        self.selected_history = Some(0);
                    }

                    let new_input = cache
                        .history
                        .get(self.selected_history.unwrap())
                        .unwrap()
                        .clone();

                    self.replace_input(&new_input);
                } else {
                    self.input_content.perform(text_editor::Action::Move(
                        text_editor::Motion::DocumentStart,
                    ));
                }

                (Task::none(), None)
            }
            Message::Down(shift) => {
                if !shift && self.completion.arrow(Arrow::Down) {
                    return (Task::none(), None);
                }

                if shift {
                    self.input_content.perform(text_editor::Action::Select(
                        text_editor::Motion::DocumentEnd,
                    ));

                    return (Task::none(), None);
                }

                let cache = history.input(convo_id);

                if let Some(index) = self.selected_history.as_mut() {
                    let new_input = if *index == 0 {
                        self.selected_history = None;
                        cache.draft_message.to_string()
                    } else {
                        *index -= 1;
                        cache.history.get(*index).unwrap().clone()
                    };

                    self.replace_input(&new_input);
                } else {
                    self.input_content.perform(text_editor::Action::Move(
                        text_editor::Motion::DocumentEnd,
                    ));
                }

                (Task::none(), None)
            }
            // Capture escape so that closing a picker or context menu does
            // not defocus the input
            Message::Escape => {
                self.completion.close_picker();

                (Task::none(), None)
            }
            Message::Paste => {
                let task = clipboard::read_text().then(|content| {
                    content.map_or_else(
                        |_| Task::none(),
                        |content| {
                            Task::done(Message::Action(
                                text_editor::Action::Edit(
                                    text_editor::Edit::Paste(content),
                                ),
                            ))
                        },
                    )
                });

                Self::close_context_menu(vec![task])
            }
            Message::Cut => {
                let task =
                    if let Some(selection) = self.input_content.selection() {
                        self.input_content.perform(text_editor::Action::Edit(
                            text_editor::Edit::Delete,
                        ));

                        clipboard::write(selection.to_string()).discard()
                    } else {
                        Task::none()
                    };

                Self::close_context_menu(vec![task])
            }
            Message::Copy => {
                let task = if let Some(input) = self.input_content.selection() {
                    clipboard::write(input.to_string()).discard()
                } else {
                    Task::none()
                };

                Self::close_context_menu(vec![task])
            }
            Message::CopyAll => {
                let input = self.input_content.text();
                let task = clipboard::write(input.to_string()).discard();

                Self::close_context_menu(vec![task])
            }
            Message::SelectAll => {
                self.input_content.perform(text_editor::Action::SelectAll);

                Self::close_context_menu(vec![])
            }
            Message::Kill(kill, save_to_clipboard) => {
                let task = text_editor_key_bindings::perform_kill(
                    &mut self.input_content,
                    kill,
                    save_to_clipboard,
                    config.buffer.text_input.kill_to_clipboard,
                );

                (task, None)
            }
        }
    }

    fn replace_input(&mut self, new_input: &str) {
        self.input_content = text_editor::Content::with_text(new_input);
        self.input_content.perform(text_editor::Action::Move(
            text_editor::Motion::DocumentEnd,
        ));
    }

    /// The caret's byte offset within the full input text, plus whether a
    /// selection is active.
    fn cursor(&self) -> (usize, bool) {
        let cursor = self.input_content.cursor();

        let position = (0..cursor.position.line)
            .filter_map(|line| self.input_content.line(line))
            .map(|line| line.text.len() + 1)
            .sum::<usize>()
            + cursor.position.column;

        (position, cursor.selection.is_some())
    }

    fn apply_completion(
        &mut self,
        entry: &completion::Entry,
        convo_id: &ConvoId,
        history: &mut history::Manager,
    ) {
        let input = self.input_content.text();
        let (cursor_position, _) = self.cursor();

        for action in entry.complete_input(&input, cursor_position) {
            self.input_content.perform(action);
        }

        history.record_draft(RawInput {
            convo_id: convo_id.clone(),
            text: self.input_content.text(),
        });
    }

    fn close_context_menu(
        mut tasks: Vec<Task<Message>>,
    ) -> (Task<Message>, Option<Event>) {
        tasks.push(context_menu::close(std::convert::identity).discard());

        (Task::batch(tasks), None)
    }

    /// Restores an unsent draft (e.g. after a failed send) when the
    /// composer is empty.
    pub fn restore_draft(&mut self, content: &str) {
        if self.input_content.text().trim().is_empty() {
            self.replace_input(content);
        }
    }

    pub fn close_picker(&mut self) -> bool {
        self.completion.close_picker()
    }

    pub fn focus(&self) -> Task<Message> {
        iced::advanced::widget::operate(
            iced::advanced::widget::operation::focusable::focus(
                self.input_id.clone(),
            ),
        )
    }

    pub fn reset(&mut self) {
        self.notice = None;
        self.selected_history = None;
        self.completion.reset();
    }
}
