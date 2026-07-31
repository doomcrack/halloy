//! Identity avatars: initials or a group glyph on a deterministic
//! gradient tile. The ramp index is the FNV-1a hash of the identity's
//! short label (`data::address::avatar_ramp`), seeded exactly like the
//! QML client: conversation tiles hash the conversation id, this
//! account's own tile takes the brand ramp (QML `Avatar` parity).

use data::appearance::theme::{
    AVATAR_RAMP_COUNT, avatar_ink, avatar_ramp_colors, avatar_self_colors,
};
use data::conversation::Kind;
use iced::widget::text::LineHeight;
use iced::widget::{Space, center, container, stack};
use iced::{Background, Border, Length, Radians};

use super::Element;
use crate::{Theme, font, icon};

/// A group tile is squared off with this corner ratio (QML parity).
const GROUP_RADIUS_RATIO: f32 = 0.34;

/// Ramp source: hashed identities take an indexed ramp; this account's
/// own tiles take the brand ramp whatever their hash (QML `isSelf`).
#[derive(Debug, Clone, Copy)]
enum Ramp {
    Index(u32),
    Own,
}

/// A conversation's avatar: an initials circle for a direct, a squared
/// glyph tile for a group, both seeded by the conversation id (QML
/// `ConversationListModel` parity).
pub fn conversation<'a, M: 'a>(
    conversation: &data::Conversation,
    size: f32,
) -> Element<'a, M> {
    let ramp = Ramp::Index(data::address::avatar_ramp(
        conversation.id.short_label(),
        AVATAR_RAMP_COUNT,
    ));

    match conversation.kind {
        Kind::Direct => tile(
            initials_text(
                data::address::initials(conversation.id.as_str()),
                size,
            ),
            ramp,
            size / 2.0,
            size,
            None,
        ),
        Kind::Group => tile(
            group_glyph(size),
            ramp,
            (size * GROUP_RADIUS_RATIO).round(),
            size,
            None,
        ),
    }
}

/// A roster member's avatar: initials seeded by the member's address,
/// this account's own rows on the brand ramp (QML `MemberListModel`
/// parity). A pending invite without a confirmed address gets a "?".
pub fn member<'a, M: 'a>(
    address: Option<&data::Address>,
    is_self: bool,
    size: f32,
) -> Element<'a, M> {
    let ramp = if is_self {
        Ramp::Own
    } else {
        Ramp::Index(data::address::avatar_ramp(
            address.map(data::Address::short_label).unwrap_or_default(),
            AVATAR_RAMP_COUNT,
        ))
    };

    tile(
        initials_text(
            address.map_or_else(|| "?".to_string(), data::Address::initials),
            size,
        ),
        ramp,
        size / 2.0,
        size,
        None,
    )
}

/// This account's own avatar: brand ramp plus a presence dot (QML
/// `AccountCard` parity). Falls back to a blank tile before `Ready`.
pub fn own<'a, M: 'a>(
    address: Option<&data::Address>,
    online: bool,
    size: f32,
) -> Element<'a, M> {
    tile(
        initials_text(
            address.map(data::Address::initials).unwrap_or_default(),
            size,
        ),
        Ramp::Own,
        size / 2.0,
        size,
        Some(online),
    )
}

fn tile<'a, M: 'a>(
    content: Element<'a, M>,
    ramp: Ramp,
    radius: f32,
    size: f32,
    presence: Option<bool>,
) -> Element<'a, M> {
    let base = center(content)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |theme: &Theme| {
            let styles = theme.styles();
            let (start, end) = match ramp {
                Ramp::Index(index) => avatar_ramp_colors(styles, index),
                Ramp::Own => avatar_self_colors(styles),
            };

            container::Style {
                background: Some(Background::Gradient(
                    iced::gradient::Linear::new(Radians(std::f32::consts::PI))
                        .add_stop(0.0, start)
                        .add_stop(1.0, end)
                        .into(),
                )),
                border: Border {
                    radius: radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        });

    let Some(online) = presence else {
        return base.into();
    };

    let dot_size = (size * 0.3).round().max(8.0);
    let dot = container(Space::new())
        .width(dot_size)
        .height(dot_size)
        .style(move |theme: &Theme| {
            let styles = theme.styles();

            container::Style {
                background: Some(
                    if online {
                        styles.text.success.color
                    } else {
                        styles.text.tertiary.color
                    }
                    .into(),
                ),
                border: Border {
                    radius: (dot_size / 2.0).into(),
                    width: 2.0,
                    color: styles.general.background,
                },
                ..Default::default()
            }
        });

    stack![
        base,
        container(dot)
            .align_right(Length::Fill)
            .align_bottom(Length::Fill)
    ]
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .into()
}

fn initials_text<'a, M: 'a>(initials: String, size: f32) -> Element<'a, M> {
    super::text(initials)
        .size((size * 0.36).round().max(9.0))
        .line_height(LineHeight::Relative(1.0))
        .font(font::MONO_BOLD.clone())
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(avatar_ink(theme.styles())),
        })
        .into()
}

fn group_glyph<'a, M: 'a>(size: f32) -> Element<'a, M> {
    icon::people()
        .size((size * 0.45).round().max(8.0))
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(avatar_ink(theme.styles())),
        })
        .into()
}
