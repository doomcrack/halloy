use serde::{Deserialize, Serialize};

use crate::config;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub member_list: MemberList,
    pub description_banner: DescriptionBanner,
}

impl From<config::buffer::Conversation> for Settings {
    fn from(config: config::buffer::Conversation) -> Self {
        Self {
            member_list: MemberList {
                enabled: config.member_list.enabled,
            },
            description_banner: DescriptionBanner {
                enabled: config.description_banner.enabled,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct MemberList {
    pub enabled: bool,
}

impl Default for MemberList {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl MemberList {
    pub fn toggle_visibility(&mut self) {
        self.enabled = !self.enabled;
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
pub struct DescriptionBanner {
    pub enabled: bool,
}

impl DescriptionBanner {
    pub fn toggle_visibility(&mut self) {
        self.enabled = !self.enabled;
    }
}
