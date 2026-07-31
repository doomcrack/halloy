use serde::{Deserialize, Deserializer};

use crate::dashboard::{BufferAction, BufferFocusedAction};

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Actions {
    pub sidebar: Sidebar,
    pub buffer: Buffer,
    #[serde(alias = "nicklist")]
    pub member_list: MemberList,
    pub notification: Notification,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Buffer {
    #[serde(alias = "click_nickname")]
    pub click_username: UsernameClickAction,
    #[serde(alias = "local")]
    pub open_internal: BufferAction,
    pub message_user: BufferAction,
    pub only_contract_expanded_message: bool,
    pub click_image_url: ImageClickAction,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            click_username: UsernameClickAction::default(),
            open_internal: BufferAction::default(),
            message_user: BufferAction::default(),
            only_contract_expanded_message: true,
            click_image_url: ImageClickAction::default(),
        }
    }
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Sidebar {
    pub buffer: BufferAction,
    pub focused_buffer: Option<BufferFocusedAction>,
    pub cycle: CycleAction,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct MemberList {
    #[serde(alias = "click_nickname")]
    pub click_username: Option<UsernameClickAction>,
}

/// What clicking a sender/member does: open a DM buffer, insert the
/// address into the composer, or nothing.
#[derive(Debug, Copy, Clone)]
pub enum UsernameClickAction {
    OpenDirect(BufferAction),
    InsertAddress,
    Noop,
}

impl Default for UsernameClickAction {
    fn default() -> Self {
        Self::OpenDirect(BufferAction::default())
    }
}

impl<'de> Deserialize<'de> for UsernameClickAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case")]
        enum ClickAction {
            #[serde(alias = "open-query")]
            OpenDirect(BufferAction),
            #[serde(alias = "insert-nickname")]
            InsertAddress,
            #[serde(alias = "no-action")]
            Noop,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Action {
            ClickAction(ClickAction),
            BufferAction(BufferAction),
        }

        match Action::deserialize(deserializer)? {
            Action::ClickAction(click_action) => match click_action {
                ClickAction::OpenDirect(buffer_action) => {
                    Ok(UsernameClickAction::OpenDirect(buffer_action))
                }
                ClickAction::InsertAddress => {
                    Ok(UsernameClickAction::InsertAddress)
                }
                ClickAction::Noop => Ok(UsernameClickAction::Noop),
            },
            Action::BufferAction(buffer_action) => {
                Ok(UsernameClickAction::OpenDirect(buffer_action))
            }
        }
    }
}

#[derive(Debug, Copy, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Notification {
    pub default: NotificationAction,
    pub open_buffer: BufferAction,
}

#[derive(Debug, Copy, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationAction {
    OpenBuffer,
    #[default]
    ActivateApplication,
}

#[derive(Debug, Copy, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageClickAction {
    #[default]
    OpenUrl,
    Preview,
}

#[derive(Debug, Copy, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CycleAction {
    #[default]
    IntoCollapsed,
    SkipCollapsed,
}

impl CycleAction {
    pub fn include_collapsed(&self) -> bool {
        matches!(self, Self::IntoCollapsed)
    }
}
