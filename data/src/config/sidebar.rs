use serde::{Deserialize, Deserializer};
use serde_untagged::UntaggedEnumVisitor;

use crate::config::Scrollbar;
use crate::serde::deserialize_u32_positive_integer;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Sidebar {
    pub max_width: Option<u16>,
    #[serde(deserialize_with = "deserialize_unread_indicator")]
    pub unread_indicator: UnreadIndicator,
    pub position: Position,
    pub ordering: Ordering,
    pub scrollbar: Scrollbar,
    #[serde(alias = "font_size")]
    pub secondary_font_size: Option<u8>,
    pub primary_font_size: Option<u8>,
    pub user_menu: UserMenu,
    pub collapse_button: CollapseButton,
    pub padding: Padding,
    pub spacing: Spacing,
    pub internal_buffers: InternalBuffers,
}

/// Conversation list order. Recency is the QML-parity default.
#[derive(Debug, Copy, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Ordering {
    #[default]
    Recent,
    Alpha,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct InternalBuffers {
    #[serde(deserialize_with = "deserialize_muteable_internal_buffers")]
    pub mute: Vec<InternalBuffer>,
    pub position: InternalBufferPosition,
    pub buffers: Vec<InternalBuffer>,
}

impl InternalBuffers {
    pub fn is_before_conversations(&self) -> bool {
        matches!(self.position, InternalBufferPosition::BeforeConversations)
    }
}

pub fn deserialize_muteable_internal_buffers<'de, D>(
    deserializer: D,
) -> Result<Vec<InternalBuffer>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum MuteableInternalBuffer {
        Logs,
    }

    Ok(Vec::<MuteableInternalBuffer>::deserialize(deserializer)?
        .into_iter()
        .map(|muteable_internal_buffer| match muteable_internal_buffer {
            MuteableInternalBuffer::Logs => InternalBuffer::Logs,
        })
        .collect())
}

#[derive(Debug, Copy, Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum InternalBufferPosition {
    #[serde(alias = "before-servers")]
    BeforeConversations,
    #[default]
    #[serde(alias = "after-servers")]
    AfterConversations,
}

#[derive(Debug, Copy, Clone, Deserialize)]
#[serde(default)]
pub struct Padding {
    pub buffer: [u16; 2],
}

impl Default for Padding {
    fn default() -> Self {
        Self { buffer: [5, 4] }
    }
}

#[derive(Debug, Copy, Clone, Deserialize)]
#[serde(default)]
pub struct Spacing {
    #[serde(alias = "server")]
    pub section: u32,
}

impl Default for Spacing {
    fn default() -> Self {
        Self { section: 2 }
    }
}

#[derive(Debug, Copy, Clone, Deserialize)]
#[serde(default)]
pub struct UserMenu {
    pub enabled: bool,
}

impl Default for UserMenu {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Copy, Clone, Deserialize)]
#[serde(default)]
pub struct CollapseButton {
    pub enabled: bool,
}

impl Default for CollapseButton {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UnreadIndicator {
    pub title: bool,
    pub icon: Icon,
    #[serde(deserialize_with = "deserialize_u32_positive_integer")]
    pub icon_size: u32,
    pub show_on_open_buffers: bool,
}

impl Default for UnreadIndicator {
    fn default() -> Self {
        UnreadIndicator {
            title: false,
            icon: Icon::Dot,
            icon_size: 6,
            show_on_open_buffers: true,
        }
    }
}

impl UnreadIndicator {
    pub fn has_icon(&self) -> bool {
        !matches!(self.icon, Icon::None)
    }
}

pub fn deserialize_unread_indicator<'de, D>(
    deserializer: D,
) -> Result<UnreadIndicator, D::Error>
where
    D: Deserializer<'de>,
{
    #[allow(clippy::redundant_closure_for_method_calls)]
    UntaggedEnumVisitor::new()
        .string(|string| match string {
            "title" => Ok(UnreadIndicator {
                title: true,
                icon: Icon::None,
                ..UnreadIndicator::default()
            }),
            "none" => Ok(UnreadIndicator {
                title: false,
                icon: Icon::None,
                ..UnreadIndicator::default()
            }),
            "dot" => Ok(UnreadIndicator {
                title: false,
                icon: Icon::Dot,
                ..UnreadIndicator::default()
            }),
            _ => Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(string),
                &"one of: \"dot\", \"title\", or \"none\"",
            )),
        })
        .map(|map| map.deserialize())
        .deserialize(deserializer)
}

#[derive(Debug, Copy, Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Icon {
    #[default]
    Dot,
    CircleEmpty,
    DotCircled,
    Certificate,
    Asterisk,
    Speaker,
    Lightbulb,
    Star,
    None,
}

#[derive(Debug, Copy, Clone, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    #[default]
    Left,
    Right,
    Top,
    Bottom,
}

impl Position {
    pub fn is_horizontal(&self) -> bool {
        match self {
            Position::Left | Position::Right => false,
            Position::Top | Position::Bottom => true,
        }
    }
}

#[derive(Debug, Copy, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InternalBuffer {
    ConfigEditor,
    Logs,
}
