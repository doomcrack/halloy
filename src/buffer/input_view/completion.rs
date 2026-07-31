use std::iter;
use std::ops::RangeInclusive;

use data::Config;
use data::buffer::SkinTone;
use iced::Length;
use iced::widget::text::Shaping;
use iced::widget::{button, column, container, row, text_editor};
use unicode_segmentation::UnicodeSegmentation;

use crate::widget::{Element, double_pass, text};
use crate::{Theme, emoji, font, theme};

const MAX_SHOWN_COMMAND_ENTRIES: usize = 6;
const MAX_SHOWN_EMOJI_ENTRIES: usize = 8;

/// The composer command set: the six Logos slash commands with their
/// argument hints. ISUPPORT/nick/channel completion died with IRC.
const COMMAND_LIST: &[Command] = &[
    Command {
        title: "dm",
        args: "[address]",
        description: "Open a direct conversation with an address",
    },
    Command {
        title: "group",
        args: "[name] [description]",
        description: "Create a group conversation",
    },
    Command {
        title: "add",
        args: "<address>",
        description: "Add a member to this group conversation",
    },
    Command {
        title: "nick",
        args: "[name]",
        description: "Set a local nickname for this conversation",
    },
    Command {
        title: "details",
        args: "",
        description: "Toggle the details panel",
    },
    Command {
        title: "clear",
        args: "",
        description: "Clear this buffer's messages",
    },
];

#[derive(Debug, Clone, Default)]
pub struct Completion {
    commands: Commands,
    emojis: Emojis,
}

impl Completion {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Process input and update the completion state
    pub fn process(
        &mut self,
        input: &str,
        cursor_position: usize,
        cursor_is_selection: bool,
        config: &Config,
    ) {
        if input.starts_with('/') {
            self.commands.process(input);

            // Disallow other completions when selecting a command
            if matches!(self.commands, Commands::Selecting { .. }) {
                self.emojis = Emojis::default();
                return;
            }
        } else {
            self.commands = Commands::default();
        }

        // If the text input has a selection, then don't show pickers
        if cursor_is_selection {
            self.emojis = Emojis::default();
            return;
        }

        if let Some(shortcode) = (config.buffer.emojis.show_picker
            || config.buffer.emojis.auto_replace)
            .then(|| {
                get_word(input, cursor_position)
                    .filter(|word| word.starts_with(':'))
            })
            .flatten()
        {
            self.emojis.process(shortcode, config);
        } else {
            self.emojis = Emojis::default();
        }
    }

    pub fn select(&mut self, config: &Config) -> Option<Entry> {
        self.commands
            .select()
            .map(Entry::Command)
            .or(self.emojis.select(config).map(Entry::Emoji))
    }

    pub fn select_at(
        &mut self,
        index: usize,
        config: &Config,
    ) -> Option<Entry> {
        self.commands
            .select_at(index)
            .map(Entry::Command)
            .or(self.emojis.select_at(index, config).map(Entry::Emoji))
    }

    /// The auto-replace path: a fully typed `:shortcode:` resolves without
    /// the picker.
    pub fn complete_emoji(
        &self,
        input: &str,
        cursor_position: usize,
    ) -> Option<Vec<text_editor::Action>> {
        if let Emojis::Selected { emoji } = self.emojis {
            Some(replace_word_with_text(input, cursor_position, emoji, None))
        } else {
            None
        }
    }

    pub fn tab(&mut self, reverse: bool, config: &Config) -> Option<Entry> {
        self.commands
            .tab(reverse)
            .or_else(|| self.emojis.tab(reverse, config))
    }

    pub fn arrow(&mut self, arrow: Arrow) -> bool {
        let reverse = match arrow {
            Arrow::Up => true,
            Arrow::Down => false,
        };

        self.commands.cycle(reverse) || self.emojis.cycle(reverse)
    }

    pub fn view<'a, Message: Clone + 'a>(
        &self,
        config: &Config,
        theme: &'a Theme,
        on_select: impl Fn(usize) -> Message + Copy + 'a,
    ) -> Option<Element<'a, Message>> {
        let command_view = self.commands.view(theme, on_select);
        let emojis_view = self.emojis.view(config, on_select);

        if command_view.is_some() || emojis_view.is_some() {
            Some(column![emojis_view, command_view].spacing(4).into())
        } else {
            None
        }
    }

    pub fn close_picker(&mut self) -> bool {
        if matches!(self.commands, Commands::Selecting { .. }) {
            self.commands = Commands::Idle;

            return true;
        } else if matches!(self.emojis, Emojis::Selecting { .. }) {
            self.emojis = Emojis::Idle;

            return true;
        }

        false
    }
}

#[derive(Debug, Clone)]
pub enum Entry {
    Command(&'static Command),
    Emoji(String),
}

impl Entry {
    pub fn complete_input(
        &self,
        input: &str,
        cursor_position: usize,
    ) -> Vec<text_editor::Action> {
        match self {
            Entry::Command(command) => vec![
                text_editor::Action::SelectAll,
                text_editor::Action::Edit(text_editor::Edit::Paste(
                    std::sync::Arc::new(format!("/{} ", command.title)),
                )),
            ],
            Entry::Emoji(emoji) => {
                replace_word_with_text(input, cursor_position, emoji, None)
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Command {
    title: &'static str,
    args: &'static str,
    description: &'static str,
}

#[derive(Debug, Clone, Default)]
enum Commands {
    #[default]
    Idle,
    Selecting {
        highlighted: Option<usize>,
        filtered: Vec<&'static Command>,
    },
    Selected {
        command: &'static Command,
    },
}

impl Commands {
    fn process(&mut self, input: &str) {
        let Some(rest) = input.strip_prefix('/') else {
            *self = Self::Idle;
            return;
        };

        let head = rest.split_whitespace().next().unwrap_or("").to_lowercase();

        // A fully typed command shows its usage hint; Enter executes it.
        if let Some(command) =
            COMMAND_LIST.iter().find(|command| command.title == head)
        {
            *self = Self::Selected { command };
            return;
        }

        // Arguments after an unknown command head can't be completed.
        if rest.contains(char::is_whitespace) {
            *self = Self::Idle;
            return;
        }

        let filtered = COMMAND_LIST
            .iter()
            .filter(|command| command.title.starts_with(&head))
            .collect::<Vec<_>>();

        if filtered.is_empty() {
            *self = Self::Idle;
        } else {
            *self = Self::Selecting {
                highlighted: match self {
                    Self::Selecting { highlighted, .. } => {
                        highlighted.filter(|index| *index < filtered.len())
                    }
                    Self::Idle | Self::Selected { .. } => None,
                },
                filtered,
            };
        }
    }

    fn select(&mut self) -> Option<&'static Command> {
        let index = if let Self::Selecting { highlighted, .. } = self {
            highlighted.unwrap_or(0)
        } else {
            return None;
        };

        self.select_at(index)
    }

    fn select_at(&mut self, index: usize) -> Option<&'static Command> {
        if let Self::Selecting { filtered, .. } = self
            && let Some(command) = filtered.get(index).copied()
        {
            *self = Self::Selected { command };

            return Some(command);
        }

        None
    }

    fn tab(&mut self, reverse: bool) -> Option<Entry> {
        if let Self::Selecting {
            highlighted,
            filtered,
        } = self
        {
            selecting_tab(highlighted, filtered, reverse);

            highlighted.and_then(|index| {
                filtered.get(index).copied().map(Entry::Command)
            })
        } else {
            None
        }
    }

    fn cycle(&mut self, reverse: bool) -> bool {
        if let Self::Selecting {
            highlighted,
            filtered,
        } = self
        {
            selecting_tab(highlighted, filtered, reverse);

            true
        } else {
            false
        }
    }

    fn view<'a, Message: Clone + 'a>(
        &self,
        theme: &'a Theme,
        on_select: impl Fn(usize) -> Message + Copy + 'a,
    ) -> Option<Element<'a, Message>> {
        match self {
            Self::Idle => None,
            Self::Selecting {
                highlighted,
                filtered,
            } => {
                let skip = {
                    let index = highlighted.unwrap_or(0);

                    let to = index.max(MAX_SHOWN_COMMAND_ENTRIES - 1);
                    to.saturating_sub(MAX_SHOWN_COMMAND_ENTRIES - 1)
                };

                let entries = filtered
                    .iter()
                    .enumerate()
                    .skip(skip)
                    .take(MAX_SHOWN_COMMAND_ENTRIES)
                    .collect::<Vec<_>>();

                let content = |width| {
                    column(entries.iter().map(|(index, command)| {
                        let selected = Some(*index) == *highlighted;

                        let title = text(format!("/{}", command.title));
                        let args = (!command.args.is_empty()).then(|| {
                            text(command.args)
                                .style(theme::text::secondary)
                                .font_maybe(
                                    theme::font_style::secondary(theme)
                                        .map(font::get),
                                )
                        });

                        Element::from(
                            button(
                                row![title, args]
                                    .spacing(6)
                                    .align_y(iced::Alignment::Center),
                            )
                            .width(width)
                            .padding(6)
                            .style(move |theme, status| {
                                theme::button::picker(theme, status, selected)
                            })
                            .on_press(on_select(*index)),
                        )
                    }))
                };

                (!entries.is_empty()).then(|| {
                    let first_pass = content(Length::Shrink);
                    let second_pass = content(Length::Fill);

                    container(double_pass(first_pass, second_pass))
                        .padding(4)
                        .style(theme::container::tooltip)
                        .width(Length::Shrink)
                        .into()
                })
            }
            Self::Selected { command } => {
                let usage = if command.args.is_empty() {
                    format!("/{}", command.title)
                } else {
                    format!("/{} {}", command.title, command.args)
                };

                Some(
                    container(
                        row![
                            text(usage),
                            text(command.description)
                                .style(theme::text::secondary)
                                .font_maybe(
                                    theme::font_style::secondary(theme)
                                        .map(font::get),
                                ),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    )
                    .padding(8)
                    .style(theme::container::tooltip)
                    .width(Length::Shrink)
                    .into(),
                )
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
enum Emojis {
    #[default]
    Idle,
    Selecting {
        highlighted: Option<usize>,
        filtered: Vec<String>,
    },
    Selected {
        emoji: &'static str,
    },
}

impl Emojis {
    fn process(&mut self, input_shortcode: &str, config: &Config) {
        let input_shortcode = input_shortcode.strip_prefix(":").unwrap_or("");

        if input_shortcode.len()
            < config.buffer.emojis.characters_to_trigger_picker
        {
            *self = Self::default();
            return;
        }

        if let Some(shortcode) = config
            .buffer
            .emojis
            .auto_replace
            .then(|| input_shortcode.strip_suffix(":"))
            .flatten()
            .map(str::to_lowercase)
        {
            if let Some(emoji) =
                pick_emoji(&shortcode, config.buffer.emojis.skin_tone)
            {
                *self = Emojis::Selected { emoji };

                return;
            }
        } else if !config.buffer.emojis.show_picker {
            *self = Self::default();
            return;
        }

        let input_shortcode = input_shortcode
            .strip_suffix(":")
            .unwrap_or(input_shortcode)
            .to_lowercase();

        *self = Emojis::Selecting {
            highlighted: None,
            filtered: emoji::matching_shortcodes(&input_shortcode),
        };
    }

    fn select(&mut self, config: &Config) -> Option<String> {
        let index = if let Self::Selecting { highlighted, .. } = self {
            highlighted.unwrap_or(0)
        } else {
            return None;
        };

        self.select_at(index, config)
    }

    fn select_at(&mut self, index: usize, config: &Config) -> Option<String> {
        if let Self::Selecting { filtered, .. } = self
            && index < filtered.len()
        {
            let shortcode = filtered.swap_remove(index);
            *self = Self::Idle;
            return pick_emoji(&shortcode, config.buffer.emojis.skin_tone)
                .map(ToString::to_string);
        }

        None
    }

    fn tab(&mut self, reverse: bool, config: &Config) -> Option<Entry> {
        if let Self::Selecting {
            highlighted,
            filtered,
        } = self
        {
            selecting_tab(highlighted, filtered, reverse);

            highlighted.and_then(|index| {
                filtered
                    .get(index)
                    .and_then(|shortcode| {
                        pick_emoji(shortcode, config.buffer.emojis.skin_tone)
                    })
                    .map(|emoji| Entry::Emoji(emoji.to_string()))
            })
        } else {
            None
        }
    }

    fn cycle(&mut self, reverse: bool) -> bool {
        if let Self::Selecting {
            highlighted,
            filtered,
        } = self
        {
            selecting_tab(highlighted, filtered, reverse);

            true
        } else {
            false
        }
    }

    fn view<'a, Message: Clone + 'a>(
        &self,
        config: &Config,
        on_select: impl Fn(usize) -> Message + Copy + 'a,
    ) -> Option<Element<'a, Message>> {
        match self {
            Self::Idle | Self::Selected { .. } => None,
            Self::Selecting {
                highlighted,
                filtered,
            } => {
                let skip = {
                    let index = highlighted.unwrap_or(0);

                    let to = index.max(MAX_SHOWN_EMOJI_ENTRIES - 1);
                    to.saturating_sub(MAX_SHOWN_EMOJI_ENTRIES - 1)
                };

                let entries = filtered
                    .iter()
                    .enumerate()
                    .skip(skip)
                    .take(MAX_SHOWN_EMOJI_ENTRIES)
                    .collect::<Vec<_>>();

                let content = |width| {
                    column(entries.iter().map(|(index, shortcode)| {
                        let selected = Some(*index) == *highlighted;
                        let content = text(format!(
                            "{} :{}:",
                            pick_emoji(
                                shortcode,
                                config.buffer.emojis.skin_tone
                            )
                            .unwrap_or(" "),
                            shortcode
                        ))
                        .shaping(Shaping::Advanced);

                        Element::from(
                            button(content)
                                .width(width)
                                .padding(6)
                                .style(move |theme, status| {
                                    theme::button::picker(
                                        theme, status, selected,
                                    )
                                })
                                .on_press(on_select(*index)),
                        )
                    }))
                };

                (!entries.is_empty()).then(|| {
                    let first_pass = content(Length::Shrink);
                    let second_pass = content(Length::Fill);

                    container(double_pass(first_pass, second_pass))
                        .padding(4)
                        .style(theme::container::tooltip)
                        .width(Length::Shrink)
                        .into()
                })
            }
        }
    }
}

fn pick_emoji(shortcode: &str, skin_tone: SkinTone) -> Option<&'static str> {
    emoji::get_by_shortcode(shortcode).map(|emoji| {
        if let Some(emoji_with_skin_tone) =
            emoji.with_skin_tone(skin_tone.into())
        {
            emoji_with_skin_tone
        } else {
            emoji
        }
        .as_str()
    })
}

fn replace_word_with_text(
    input: &str,
    cursor_position: usize,
    text: &str,
    suffix: Option<&str>,
) -> Vec<text_editor::Action> {
    let mut actions: Vec<text_editor::Action> = vec![];

    let append_suffix = if cursor_position == input.len() {
        if let Some((last_word_position, last_word)) = input
            .split(' ')
            .rev()
            .enumerate()
            .find(|(_, word)| !word.is_empty())
        {
            actions.extend(iter::repeat_n(
                text_editor::Action::Select(text_editor::Motion::Left),
                last_word_position
                    + UnicodeSegmentation::graphemes(last_word, true).count(),
            ));
        }

        true
    } else {
        let mut previous_word_bounds = Option::<RangeInclusive<usize>>::None;

        let mut append_suffix = false;

        for word in input.split(' ') {
            let word_bounds =
                if let Some(previous_word_bounds) = previous_word_bounds {
                    RangeInclusive::new(
                        previous_word_bounds.end() + 1,
                        previous_word_bounds.end() + 1 + word.len(),
                    )
                } else {
                    RangeInclusive::new(0, word.len())
                };

            if word_bounds.contains(&cursor_position) {
                let mut byte_position = *word_bounds.start();
                let graphemes_to_the_left =
                    UnicodeSegmentation::graphemes(word, true)
                        .take_while(|grapheme| {
                            if byte_position < cursor_position {
                                byte_position += grapheme.len();
                                true
                            } else {
                                false
                            }
                        })
                        .count();

                let mut byte_position = *word_bounds.end();
                let graphemes_to_the_right =
                    UnicodeSegmentation::graphemes(word, true)
                        .rev()
                        .take_while(|grapheme| {
                            if byte_position > cursor_position {
                                byte_position -= grapheme.len();
                                true
                            } else {
                                false
                            }
                        })
                        .count();

                if graphemes_to_the_left <= graphemes_to_the_right {
                    actions.extend(iter::repeat_n(
                        text_editor::Action::Move(text_editor::Motion::Left),
                        graphemes_to_the_left,
                    ));

                    actions.extend(iter::repeat_n(
                        text_editor::Action::Select(text_editor::Motion::Right),
                        UnicodeSegmentation::graphemes(word, true).count(),
                    ));
                } else {
                    actions.extend(iter::repeat_n(
                        text_editor::Action::Move(text_editor::Motion::Right),
                        graphemes_to_the_right,
                    ));

                    actions.extend(iter::repeat_n(
                        text_editor::Action::Select(text_editor::Motion::Left),
                        UnicodeSegmentation::graphemes(word, true).count(),
                    ));
                }

                if let Some(suffix) = suffix {
                    append_suffix = input.get(*word_bounds.end()..).is_none_or(
                        |after_word| !after_word.starts_with(suffix),
                    );
                }

                break;
            }

            previous_word_bounds = Some(word_bounds);
        }

        append_suffix
    };

    actions.push(text_editor::Action::Edit(text_editor::Edit::Paste(
        std::sync::Arc::new(text.to_string()),
    )));

    if let Some(suffix) = suffix
        && append_suffix
    {
        actions.push(text_editor::Action::Edit(text_editor::Edit::Paste(
            std::sync::Arc::new(suffix.to_string()),
        )));
    }

    actions
}

fn selecting_tab<T>(
    highlighted: &mut Option<usize>,
    filtered: &[T],
    reverse: bool,
) {
    if filtered.is_empty() {
        *highlighted = None;
    } else if let Some(index) = highlighted {
        if reverse {
            if *index > 0 {
                *index -= 1;
            } else {
                *index = filtered.len() - 1;
            }
        } else {
            *index = (*index + 1) % filtered.len();
        }
    } else {
        *highlighted = Some(if reverse { filtered.len() - 1 } else { 0 });
    }
}

pub enum Arrow {
    Up,
    Down,
}

fn get_word(input: &str, cursor_position: usize) -> Option<&str> {
    get_word_bounds(input, cursor_position).and_then(|word_bounds| {
        input.get(*word_bounds.start()..*word_bounds.end())
    })
}

fn get_word_bounds(
    input: &str,
    cursor_position: usize,
) -> Option<RangeInclusive<usize>> {
    let mut previous_word_bounds = Option::<RangeInclusive<usize>>::None;

    if cursor_position == input.len() {
        let mut trailing_spaces = 0;

        let word_bounds_start = input
            .split(' ')
            .rfind(|word| {
                if word.is_empty() {
                    trailing_spaces += 1;
                    false
                } else {
                    true
                }
            })
            .map(|word| {
                input.len().saturating_sub(word.len() + trailing_spaces)
            });

        return word_bounds_start.map(|word_bounds_start| {
            RangeInclusive::new(
                word_bounds_start,
                input.len().saturating_sub(trailing_spaces),
            )
        });
    }

    for word in input.split(' ') {
        let word_bounds =
            if let Some(previous_word_bounds) = previous_word_bounds {
                RangeInclusive::new(
                    previous_word_bounds.end() + 1,
                    previous_word_bounds.end() + 1 + word.len(),
                )
            } else {
                RangeInclusive::new(0, word.len())
            };

        if word_bounds.contains(&cursor_position) {
            return Some(word_bounds);
        }

        previous_word_bounds = Some(word_bounds);
    }

    None
}
