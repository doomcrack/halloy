use std::collections::HashMap;
use std::thread;

use chrono::{DateTime, TimeDelta, Utc};
use data::audio::Sound;
use data::buffer::Buffer;
use data::config::actions::NotificationAction;
use data::config::notification;
use data::{Config, Notification};
use iced::Task;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use self::toast::Toast;
pub use self::toast::prepare;
use crate::audio;

pub mod toast;

#[derive(Debug)]
pub enum Event {
    RequestAttention {
        buffer: Option<Buffer>,
    },
    NotificationResponse {
        action: toast::Action,
        buffer: Option<Buffer>,
    },
}

#[derive(PartialEq, Eq, Hash, Clone)]
enum NotificationDelayKey {
    Connected,
    Disconnected,
    Reconnected,
    DirectMessage(Box<str>),
    GroupMessage(Box<str>),
    GroupInvite(Box<str>),
}

impl From<&Notification> for NotificationDelayKey {
    fn from(notification: &Notification) -> NotificationDelayKey {
        match notification {
            Notification::Connected => NotificationDelayKey::Connected,
            Notification::Disconnected => NotificationDelayKey::Disconnected,
            Notification::Reconnected => NotificationDelayKey::Reconnected,
            Notification::DirectMessage { sender, .. } => {
                NotificationDelayKey::DirectMessage(sender.as_str().into())
            }
            Notification::GroupMessage { convo_id, .. } => {
                NotificationDelayKey::GroupMessage(convo_id.as_str().into())
            }
            Notification::GroupInvite { convo_id, .. } => {
                NotificationDelayKey::GroupInvite(convo_id.as_str().into())
            }
        }
    }
}

pub struct Notifications {
    recent_notifications: HashMap<NotificationDelayKey, DateTime<Utc>>,
    sounds: HashMap<String, Sound>,
    sender: mpsc::Sender<Event>,
    audio: Option<thread::JoinHandle<()>>,
}

impl Notifications {
    pub fn new(config: &Config) -> (Self, Task<Event>) {
        let sounds = config.notifications.load_sounds();

        let (sender, receiver) = mpsc::channel(50);

        (
            Self {
                recent_notifications: HashMap::new(),
                sounds,
                sender,
                audio: None,
            },
            Task::stream(ReceiverStream::new(receiver)),
        )
    }

    pub fn update(&mut self, config: &Config) {
        self.sounds = config.notifications.load_sounds();
    }

    pub fn notify(&mut self, config: &Config, notification: &Notification) {
        let (notification_config, title, body, buffer) = match notification {
            Notification::Connected => (
                &config.notifications.connected,
                "Connected".to_string(),
                "Message delivery is online".to_string(),
                None,
            ),
            Notification::Disconnected => (
                &config.notifications.disconnected,
                "Disconnected".to_string(),
                "Message delivery was interrupted".to_string(),
                None,
            ),
            Notification::Reconnected => (
                &config.notifications.reconnected,
                "Reconnected".to_string(),
                "Message delivery is back online".to_string(),
                None,
            ),
            Notification::DirectMessage {
                convo_id,
                sender,
                message,
            } => {
                let body = if config.notifications.direct_message.show_content {
                    message.clone()
                } else {
                    "Sent you a direct message".to_string()
                };

                (
                    &config.notifications.direct_message,
                    sender.short_label().to_string(),
                    body,
                    Some(Buffer::Conversation(convo_id.clone())),
                )
            }
            Notification::GroupMessage {
                convo_id,
                title,
                message,
            } => {
                let body = if config.notifications.group_message.show_content {
                    message.clone()
                } else {
                    "New message".to_string()
                };

                (
                    &config.notifications.group_message,
                    title.clone(),
                    body,
                    Some(Buffer::Conversation(convo_id.clone())),
                )
            }
            Notification::GroupInvite { convo_id, title } => (
                &config.notifications.group_invite,
                title.clone(),
                "You were added to this group".to_string(),
                Some(Buffer::Conversation(convo_id.clone())),
            ),
        };

        if notification_config.request_attention {
            let sender = self.sender.clone();
            let buffer = buffer.clone();

            tokio::task::spawn(async move {
                let _ = sender.send(Event::RequestAttention { buffer }).await;
            });
        }

        self.execute(
            notification_config,
            config.actions.notification.default,
            notification,
            &title,
            &body,
            buffer,
        );
    }

    fn execute(
        &mut self,
        config: &notification::Notification,
        default_notification_action: NotificationAction,
        notification: &Notification,
        title: &str,
        body: &str,
        buffer: Option<Buffer>,
    ) {
        let now = Utc::now();
        let delay_key = notification.into();

        if self.recent_notifications.get(&delay_key).is_some_and(
            |last_notification| {
                now - last_notification
                    < TimeDelta::milliseconds(i64::from(
                        config.delay.unwrap_or(500),
                    ))
            },
        ) {
            return;
        }

        self.recent_notifications.insert(delay_key, now);

        if config.show_toast {
            let toast = Toast::new(
                title,
                None,
                body,
                buffer.is_some(),
                default_notification_action,
            );

            let sender = self.sender.clone();

            tokio::task::spawn(async move {
                if let Some(action) = toast
                    .show_and_wait_for_response(default_notification_action)
                    .await
                {
                    let _ = sender
                        .send(Event::NotificationResponse { action, buffer })
                        .await;
                }
            });
        }

        if let Some(sound) = config
            .sound
            .as_deref()
            .and_then(|sound_name| self.sounds.get(sound_name))
            && self
                .audio
                .as_ref()
                .is_none_or(thread::JoinHandle::is_finished)
        {
            self.audio = Some(audio::play(sound.clone()));
        }
    }
}
