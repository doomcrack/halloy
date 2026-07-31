use std::collections::HashMap;

use serde::Deserialize;

use crate::audio::Sound;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Notification {
    pub show_toast: bool,
    pub request_attention: bool,
    pub show_content: bool,
    pub sound: Option<String>,
    pub delay: Option<u32>,
}

impl Default for Notification {
    fn default() -> Self {
        Self {
            show_toast: false,
            request_attention: false,
            show_content: false,
            sound: None,
            delay: Some(500),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Notifications {
    pub connected: Notification,
    pub disconnected: Notification,
    pub reconnected: Notification,
    pub direct_message: Notification,
    pub group_message: Notification,
    pub group_invite: Notification,
}

impl Notifications {
    pub fn load_sounds(&self) -> HashMap<String, Sound> {
        let mut sounds = HashMap::new();

        let mut load_and_insert = |name: &str| {
            if !sounds.contains_key(name) {
                match Sound::load(name) {
                    Ok(sound) => {
                        sounds.insert(name.to_string(), sound);
                    }
                    Err(e) => {
                        log::warn!("Failed to load sound '{name}': {e}");
                    }
                }
            }
        };

        for notification in [
            &self.connected,
            &self.disconnected,
            &self.reconnected,
            &self.direct_message,
            &self.group_message,
            &self.group_invite,
        ] {
            if let Some(sound_name) = notification.sound.as_deref() {
                load_and_insert(sound_name);
            }
        }

        sounds
    }
}
