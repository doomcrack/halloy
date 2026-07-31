use chrono::format::SecondsFormat;
use chrono::{DateTime, Local, NaiveDate, Utc};
use iced::Color;
use serde::{Deserialize, Deserializer};

pub use self::conversation::Conversation;
pub use self::hide_consecutive::{HideConsecutive, HideConsecutiveEnabled};
pub use self::sender::Sender;
pub use crate::appearance::theme::{alpha_color, alpha_color_calculate};
use crate::buffer::{BacklogSeparator, DateSeparators, SkinTone, Timestamp};
use crate::config::buffer::text_input::TextInput;

pub mod conversation;
pub mod hide_consecutive;
pub mod sender;
pub mod text_input;

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Buffer {
    pub timestamp: Timestamp,
    #[serde(alias = "nickname")]
    pub sender: Sender,
    pub text_input: TextInput,
    #[serde(alias = "channel")]
    pub conversation: Conversation,
    pub backlog_separator: BacklogSeparator,
    pub date_separators: DateSeparators,
    pub emojis: Emojis,
    pub mark_as_read: MarkAsRead,
    pub url: Url,
    pub line_spacing: u32,
    pub scroll_position_on_open: ScrollPosition,
    pub close: Close,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Close {
    #[serde(alias = "query")]
    pub direct: CloseDirect,
}

/// Whether closing a direct conversation's buffer keeps it in the sidebar.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CloseDirect {
    #[default]
    Keep,
    Close,
}

impl CloseDirect {
    pub fn close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Emojis {
    pub show_picker: bool,
    pub skin_tone: SkinTone,
    pub auto_replace: bool,
    pub characters_to_trigger_picker: usize,
}

impl Default for Emojis {
    fn default() -> Self {
        Self {
            show_picker: true,
            skin_tone: SkinTone::default(),
            auto_replace: true,
            characters_to_trigger_picker: 2,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Url {
    pub prompt_before_open: bool,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScrollPosition {
    #[default]
    OldestUnread,
    Newest,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MarkAsRead {
    pub on_application_exit: bool,
    pub on_buffer_close: OnBufferClose,
    pub on_scroll_to_bottom: bool,
    pub on_message_sent: bool,
    pub on_message: OnMessage,
}

impl Default for MarkAsRead {
    fn default() -> Self {
        Self {
            on_application_exit: false,
            on_buffer_close: OnBufferClose::default(),
            on_scroll_to_bottom: true,
            on_message_sent: true,
            on_message: OnMessage::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum OnBufferClose {
    Bool(bool),
    Condition(OnBufferCloseCondition),
}

impl Default for OnBufferClose {
    fn default() -> Self {
        Self::Condition(OnBufferCloseCondition::ScrolledToBottom)
    }
}

impl OnBufferClose {
    pub fn mark_as_read(&self, is_scrolled_to_bottom: Option<bool>) -> bool {
        match self {
            OnBufferClose::Bool(mark) => *mark,
            OnBufferClose::Condition(_) => {
                is_scrolled_to_bottom.unwrap_or(false)
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnBufferCloseCondition {
    ScrolledToBottom,
}

#[derive(Debug, Default, Clone)]
pub enum OnMessage {
    #[default]
    Focused,
    Open,
    None,
}

impl<'de> Deserialize<'de> for OnMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub enum Enum {
            Focused,
            Open,
            None,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OnMessageCondition {
            Enum(Enum),
            Bool(bool),
        }

        match OnMessageCondition::deserialize(deserializer)? {
            OnMessageCondition::Enum(on_message) => Ok(match on_message {
                Enum::Focused => Self::Focused,
                Enum::Open => Self::Open,
                Enum::None => Self::None,
            }),
            OnMessageCondition::Bool(boolean) => {
                if boolean {
                    Ok(Self::Open)
                } else {
                    Ok(Self::None)
                }
            }
        }
    }
}

impl Buffer {
    pub fn format_timestamp(
        &self,
        date_time: &DateTime<Utc>,
    ) -> Option<String> {
        if self.timestamp.format.is_empty() {
            return None;
        }

        Some(self.timestamp.brackets.format(
            date_time.with_timezone(&Local).format_localized(
                &self.timestamp.format,
                self.timestamp.locale,
            ),
        ))
    }

    pub fn format_range_end_timestamp(
        &self,
        end_date_time: &DateTime<Utc>,
    ) -> Option<(String, String)> {
        if self.timestamp.format.is_empty() {
            return None;
        }

        Some((
            "\u{2013} ".to_string(),
            self.timestamp
                .brackets
                .format(format!(
                    "{}",
                    end_date_time.with_timezone(&Local).format_localized(
                        &self.timestamp.format,
                        self.timestamp.locale
                    )
                ))
                .to_string(),
        ))
    }

    pub fn format_context_menu_timestamp(
        &self,
        date_time: &DateTime<Utc>,
    ) -> String {
        date_time
            .with_timezone(&Local)
            .format_localized(
                &self.timestamp.context_menu_format,
                self.timestamp.locale,
            )
            .to_string()
    }

    pub fn format_copy_timestamp(&self, date_time: &DateTime<Utc>) -> String {
        if let Some(copy_format) = &self.timestamp.copy_format {
            date_time
                .with_timezone(&Local)
                .format_localized(copy_format, self.timestamp.locale)
                .to_string()
        } else {
            date_time
                .with_timezone(&Local)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        }
    }

    pub fn format_date_separator(&self, date: &NaiveDate) -> String {
        date.and_hms_opt(0, 0, 0)
            .and_then(|date_time| date_time.and_local_timezone(Local).single())
            .map_or(
                // in the event of timezone weirdness,
                // revert to default format
                date.format_localized(
                    &DateSeparators::default().format,
                    self.timestamp.locale,
                ),
                |date_time| {
                    date_time.format_localized(
                        &self.date_separators.format,
                        self.timestamp.locale,
                    )
                },
            )
            .to_string()
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Dimmed {
    enabled: bool,
    alpha: Option<f32>,
}

impl Default for Dimmed {
    fn default() -> Self {
        Self {
            enabled: true,
            alpha: None,
        }
    }
}

impl Dimmed {
    pub fn new(alpha: Option<f32>) -> Self {
        Dimmed {
            enabled: true,
            alpha,
        }
    }

    pub fn transform_color(&self, color: Color, background: Color) -> Color {
        if self.enabled {
            match self.alpha {
                // Calculate alpha based on background and foreground.
                None => alpha_color_calculate(0.20, 0.61, background, color),
                // Calculate alpha based on user defined alpha value.
                Some(a) => alpha_color(color, a),
            }
        } else {
            color
        }
    }
}

pub fn deserialize_dimmed_maybe<'de, D>(
    deserializer: D,
) -> Result<Option<Dimmed>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Data {
        Boolean(bool),
        Float(f32),
    }

    let dimmed_maybe: Option<Data> = Deserialize::deserialize(deserializer)?;

    Ok(dimmed_maybe.map(|dimmed| match dimmed {
        Data::Boolean(dim) => Dimmed {
            enabled: dim,
            alpha: None,
        },
        Data::Float(dim) => Dimmed {
            enabled: true,
            alpha: Some(dim),
        },
    }))
}
