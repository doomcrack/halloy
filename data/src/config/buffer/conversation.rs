use serde::Deserialize;

use crate::buffer::conversation::Position;

/// Conversation buffer defaults (the IRC-era `channel` block).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Conversation {
    #[serde(alias = "nicklist")]
    pub member_list: MemberList,
    #[serde(alias = "topic", alias = "topic_banner")]
    pub description_banner: DescriptionBanner,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MemberList {
    pub enabled: bool,
    pub position: Position,
    pub width: Option<f32>,
    pub alignment: Alignment,
    pub truncate: Option<u16>,
}

impl Default for MemberList {
    fn default() -> Self {
        Self {
            enabled: true,
            position: Position::default(),
            width: None,
            alignment: Alignment::default(),
            truncate: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Alignment {
    #[default]
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct DescriptionBanner {
    pub enabled: bool,
    pub max_lines: u16,
}

impl Default for DescriptionBanner {
    fn default() -> Self {
        Self {
            enabled: true,
            max_lines: 2,
        }
    }
}
