use std::str::FromStr;

use chrono::Locale;
use iced_core::Color as IcedColor;
use serde::{Deserialize, Deserializer, Serialize};

pub mod conversation;
pub mod timestamp;

pub use self::timestamp::Timestamp;
use crate::appearance::theme::hex_to_color;
use crate::config;
use crate::conversation::ConvoId;
use crate::serde::deserialize_strftime_date;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "kebab-case")]
pub enum Buffer {
    Conversation(ConvoId),
    Internal(Internal),
}

impl From<ConvoId> for Buffer {
    fn from(convo_id: ConvoId) -> Self {
        Self::Conversation(convo_id)
    }
}

impl From<Internal> for Buffer {
    fn from(internal: Internal) -> Self {
        Self::Internal(internal)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    strum::Display,
    strum::IntoStaticStr,
)]
pub enum Internal {
    Logs,
    #[strum(serialize = "Config Editor")]
    ConfigEditor,
}

impl Buffer {
    pub fn key(&self) -> String {
        match self {
            Buffer::Conversation(convo_id) => format!("convo:{convo_id}"),
            Buffer::Internal(internal) => internal.key(),
        }
    }

    pub fn convo_id(&self) -> Option<&ConvoId> {
        if let Self::Conversation(convo_id) = self {
            Some(convo_id)
        } else {
            None
        }
    }

    pub fn internal(&self) -> Option<&Internal> {
        if let Self::Internal(internal) = self {
            Some(internal)
        } else {
            None
        }
    }
}

impl Internal {
    pub const ALL: &'static [Self] = &[Self::Logs, Self::ConfigEditor];

    pub fn key(&self) -> String {
        match self {
            Internal::Logs => "logs",
            Internal::ConfigEditor => "config-editor",
        }
        .to_string()
    }
}

impl From<&config::sidebar::InternalBuffer> for Internal {
    fn from(config: &config::sidebar::InternalBuffer) -> Self {
        match config {
            config::sidebar::InternalBuffer::ConfigEditor => Self::ConfigEditor,
            config::sidebar::InternalBuffer::Logs => Self::Logs,
        }
    }
}

impl From<config::sidebar::InternalBuffer> for Internal {
    fn from(config: config::sidebar::InternalBuffer) -> Self {
        Self::from(&config)
    }
}

impl From<&Internal> for config::sidebar::InternalBuffer {
    fn from(buffer: &Internal) -> Self {
        match buffer {
            Internal::ConfigEditor => Self::ConfigEditor,
            Internal::Logs => Self::Logs,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub conversation: conversation::Settings,
}

impl From<config::Buffer> for Settings {
    fn from(config: config::Buffer) -> Self {
        Self {
            conversation: conversation::Settings::from(config.conversation),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Brackets {
    pub left: String,
    pub right: String,
}

impl Brackets {
    pub fn format(&self, content: impl std::fmt::Display) -> String {
        format!("{}{}{}", self.left, content, self.right)
    }
}

#[derive(Debug, Clone)]
pub enum BacklogText {
    Text(String),
    Hidden,
}

impl Default for BacklogText {
    fn default() -> Self {
        Self::Text("backlog".to_string())
    }
}

impl<'de> Deserialize<'de> for BacklogText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum T {
            String(String),
            Bool(bool),
        }

        match T::deserialize(deserializer)? {
            T::String(conf) => Ok(Self::Text(conf)),
            T::Bool(val) => {
                if !val {
                    Ok(Self::Hidden)
                } else {
                    Ok(Self::default())
                }
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BacklogSeparator {
    pub hide_when_all_read: bool,
    pub text: BacklogText,
}

impl Default for BacklogSeparator {
    fn default() -> Self {
        Self {
            hide_when_all_read: true,
            text: BacklogText::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DateSeparators {
    #[serde(deserialize_with = "deserialize_strftime_date")]
    pub format: String,
    pub show: bool,
}

impl Default for DateSeparators {
    fn default() -> Self {
        Self {
            format: "%A, %B %-d".to_string(),
            show: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub enum Color {
    Solid,
    #[default]
    Unique,
    Palette(Vec<IcedColor>),
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            String(String),
            Palette { palette: Vec<String> },
        }

        match Repr::deserialize(deserializer)? {
            Repr::String(value) => match value.as_str() {
                "solid" => Ok(Self::Solid),
                "unique" => Ok(Self::Unique),
                _ => Err(serde::de::Error::custom(format!(
                    "unknown color: {value}",
                ))),
            },
            Repr::Palette { palette } => {
                if palette.is_empty() {
                    return Err(serde::de::Error::custom(
                        "palette must contain at least one hex color",
                    ));
                }

                let colors = palette
                    .into_iter()
                    .map(|hex| {
                        hex_to_color(&hex).ok_or_else(|| {
                            serde::de::Error::custom(format!(
                                "invalid hex color in palette: {hex}",
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(Self::Palette(colors))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Alignment {
    #[default]
    Left,
    Right,
    Top,
}

impl Alignment {
    pub fn is_right(&self) -> bool {
        matches!(self, Self::Right)
    }

    pub fn is_top(&self) -> bool {
        matches!(self, Self::Top)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RightAlignmentWidths {
    pub prefixes: f32,
    pub timestamp: f32,
    pub middle: f32,
}

#[derive(Debug, Clone, Copy)]
pub enum Resize {
    None,
    Maximize,
    Restore,
}

impl Resize {
    pub fn action(can_resize: bool, maximized: bool) -> Self {
        if can_resize {
            if maximized {
                Self::Restore
            } else {
                Self::Maximize
            }
        } else {
            Self::None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkinTone {
    #[default]
    Default,
    Light,
    MediumLight,
    Medium,
    MediumDark,
    Dark,
}

impl From<SkinTone> for emojis::SkinTone {
    fn from(skin_tone: SkinTone) -> Self {
        match skin_tone {
            SkinTone::Default => emojis::SkinTone::Default,
            SkinTone::Light => emojis::SkinTone::Light,
            SkinTone::MediumLight => emojis::SkinTone::MediumLight,
            SkinTone::Medium => emojis::SkinTone::Medium,
            SkinTone::MediumDark => emojis::SkinTone::MediumDark,
            SkinTone::Dark => emojis::SkinTone::Dark,
        }
    }
}

pub fn deserialize_locale<'de, D>(deserializer: D) -> Result<Locale, D::Error>
where
    D: Deserializer<'de>,
{
    let locale_string_maybe: Option<String> =
        Deserialize::deserialize(deserializer)?;

    if let Some(locale_string) = &locale_string_maybe {
        if let Ok(locale) = Locale::from_str(&locale_string.replace("-", "_")) {
            Ok(locale)
        } else {
            Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(locale_string),
                &"IETF BCP 47 language tag",
            ))
        }
    } else {
        Ok(sys_locale::get_locale()
            .and_then(|locale_string| {
                Locale::from_str(&locale_string.replace("-", "_")).ok()
            })
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::{Buffer, Color, Internal};
    use crate::conversation::ConvoId;

    #[derive(Debug, serde::Deserialize)]
    struct Root {
        color: Color,
    }

    #[test]
    fn color_deserializes_palette() {
        let root: Root = toml::from_str(
            r##"color = { palette = ["#112233", "#445566", "#778899"] }"##,
        )
        .expect("valid palette color");

        match root.color {
            Color::Palette(colors) => assert_eq!(colors.len(), 3),
            _ => panic!("expected palette color"),
        }
    }

    #[test]
    fn color_rejects_empty_palette() {
        let err = toml::from_str::<Root>(r#"color = { palette = [] }"#)
            .expect_err("empty palette should be rejected");

        assert!(
            err.to_string()
                .contains("palette must contain at least one")
        );
    }

    #[test]
    fn color_rejects_invalid_palette_hex() {
        let err = toml::from_str::<Root>(
            r##"color = { palette = ["#112233", "not-a-color"] }"##,
        )
        .expect_err("invalid palette hex should be rejected");

        assert!(err.to_string().contains("invalid hex color in palette"));
    }

    #[test]
    fn buffer_serde_is_tagged_and_round_trips() {
        let conversation = Buffer::Conversation(ConvoId::from("logs"));
        let json = serde_json::to_string(&conversation).unwrap();
        assert_eq!(json, r#"{"type":"conversation","id":"logs"}"#);
        assert_eq!(
            serde_json::from_str::<Buffer>(&json).unwrap(),
            conversation
        );

        let internal = Buffer::Internal(Internal::Logs);
        let json = serde_json::to_string(&internal).unwrap();
        assert_eq!(json, r#"{"type":"internal","id":"Logs"}"#);
        assert_eq!(serde_json::from_str::<Buffer>(&json).unwrap(), internal);
    }

    #[test]
    fn buffer_rejects_old_untagged_format() {
        assert!(serde_json::from_str::<Buffer>(r#""logs""#).is_err());
        assert!(
            serde_json::from_str::<Buffer>(
                r#"{"Upstream":{"Server":"libera"}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn buffer_keys() {
        assert_eq!(
            Buffer::Conversation(ConvoId::from("abc123")).key(),
            "convo:abc123"
        );
        assert_eq!(Buffer::Internal(Internal::Logs).key(), "logs");
        assert_eq!(
            Buffer::Internal(Internal::ConfigEditor).key(),
            "config-editor"
        );
    }
}
