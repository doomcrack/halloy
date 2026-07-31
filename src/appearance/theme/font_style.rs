use data::appearance::theme::FontStyle;
use data::{log, message};

use super::Theme;

pub fn primary(theme: &Theme) -> Option<FontStyle> {
    theme.styles().text.primary.font_style
}

pub fn secondary(theme: &Theme) -> Option<FontStyle> {
    theme.styles().text.secondary.font_style
}

pub fn tertiary(theme: &Theme) -> Option<FontStyle> {
    theme.styles().text.tertiary.font_style
}

pub fn sender(theme: &Theme) -> Option<FontStyle> {
    theme.styles().buffer.nickname.font_style
}

pub fn status(theme: &Theme, status: message::StatusKind) -> Option<FontStyle> {
    match status {
        message::StatusKind::SendFailed => error(theme),
        message::StatusKind::NewConversation
        | message::StatusKind::MemberChange
        | message::StatusKind::Info => secondary(theme),
    }
}

pub fn error(theme: &Theme) -> Option<FontStyle> {
    theme.styles().text.error.font_style
}

pub fn timestamp(theme: &Theme) -> Option<FontStyle> {
    theme.styles().buffer.timestamp.font_style
}

pub fn url(theme: &Theme) -> Option<FontStyle> {
    theme.styles().buffer.url.font_style
}

pub fn log_level(theme: &Theme, log_level: log::Level) -> Option<FontStyle> {
    match log_level {
        log::Level::Error => theme.styles().text.error.font_style,
        log::Level::Warn => theme.styles().text.warning.font_style,
        log::Level::Info => theme.styles().text.info.font_style,
        log::Level::Debug => theme.styles().text.debug.font_style,
        log::Level::Trace => theme.styles().text.trace.font_style,
    }
}
