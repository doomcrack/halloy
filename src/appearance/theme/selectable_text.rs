use data::address::avatar_ramp;
use data::appearance::theme::{
    AVATAR_RAMP_COUNT, avatar_ramp_colors, avatar_self_colors, nickname_color,
};
use data::{Config, buffer, log, message};
use iced::Color;

use super::{Theme, text};
use crate::widget::selectable_text::{Catalog, Style, StyleFn};

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(default)
    }

    fn style(&self, class: &Self::Class<'_>) -> Style {
        class(self)
    }
}

pub fn default(theme: &Theme) -> Style {
    Style {
        color: None,
        selection_color: theme.styles().buffer.selection,
    }
}

pub fn logs(theme: &Theme) -> Style {
    Style {
        color: None,
        selection_color: theme.styles().buffer.selection,
    }
}

pub fn timestamp(theme: &Theme) -> Style {
    let color = text::timestamp(theme).color;

    Style {
        color,
        selection_color: theme.styles().buffer.selection,
    }
}

/// Sender label color; `seed` drives the per-sender color when the
/// configured color mode is `Unique`/`Palette` (pass the short label for
/// QML avatar-ramp parity). `Unique` — the default — takes the first stop
/// of the ramp the sender's avatar is drawn in, so a name and a face read
/// as the same person.
pub fn sender(theme: &Theme, config: &Config, seed: Option<&str>) -> Style {
    let color = match (&config.buffer.sender.color, seed) {
        (buffer::Color::Unique, Some(seed)) => {
            avatar_ramp_colors(
                theme.styles(),
                avatar_ramp(seed, AVATAR_RAMP_COUNT),
            )
            .0
        }
        (kind, seed) => {
            nickname_color(theme.styles().buffer.nickname.color, kind, seed)
        }
    };

    Style {
        color: Some(color),
        selection_color: theme.styles().buffer.selection,
    }
}

/// This account's own sender label: the brand ramp its own avatar takes,
/// so your own messages read as yours on every theme (QML `isMe`).
pub fn own_sender(theme: &Theme) -> Style {
    Style {
        color: Some(avatar_self_colors(theme.styles()).0),
        selection_color: theme.styles().buffer.selection,
    }
}

pub fn status(theme: &Theme, status: message::StatusKind) -> Style {
    let color = match status {
        message::StatusKind::SendFailed => text::error(theme).color,
        message::StatusKind::NewConversation
        | message::StatusKind::MemberChange
        | message::StatusKind::Info => text::secondary(theme).color,
    };

    Style {
        color,
        selection_color: theme.styles().buffer.selection,
    }
}

pub fn log_level(theme: &Theme, log_level: log::Level) -> Style {
    let color = match log_level {
        log::Level::Error => theme.styles().text.error.color,
        log::Level::Warn => theme
            .styles()
            .text
            .warning
            .color
            .unwrap_or(theme.styles().general.unread_indicator),
        log::Level::Info => theme
            .styles()
            .text
            .info
            .color
            .unwrap_or(theme.styles().buffer.server_messages.default.color),
        log::Level::Debug => theme
            .styles()
            .text
            .debug
            .color
            .unwrap_or(theme.styles().buffer.code.color),
        log::Level::Trace => theme
            .styles()
            .text
            .trace
            .color
            .unwrap_or(theme.styles().text.secondary.color),
    };

    Style {
        color: Some(color),
        selection_color: theme.styles().buffer.selection,
    }
}

pub fn color_dot(theme: &Theme, color: Color) -> Style {
    Style {
        color: Some(color),
        selection_color: theme.styles().buffer.selection,
    }
}
