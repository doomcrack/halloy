//! Group roster column: who is in the conversation, pending invitations
//! drawn back with a waiting note, and the add-member action pinned at
//! the foot (QML `MembersPane`/`MemberDelegate` parity). Rows carry a
//! context menu: copy the member's address, or open a DM with them.

use data::conversation::{Conversation, Member};
use data::{Address, Config};
use iced::widget::{
    Scrollable, button, center, column, container, row, rule, scrollable,
};
use iced::{Length, mouse, padding};

use crate::widget::{Element, avatar, context_menu, text, tooltip};
use crate::{Theme, font, icon, theme};

const AVATAR_SIZE: f32 = 24.0;
pub const DEFAULT_WIDTH: f32 = 180.0;

#[derive(Debug, Clone)]
pub enum Message {
    AddMember,
    CopyAddress(String),
    Dm(String),
}

#[derive(Debug, Clone, Copy)]
enum Entry {
    CopyAddress,
    Dm,
}

impl Entry {
    fn list(is_self: bool) -> Vec<Self> {
        if is_self {
            vec![Entry::CopyAddress]
        } else {
            vec![Entry::CopyAddress, Entry::Dm]
        }
    }
}

pub fn view<'a>(
    conversation: &'a Conversation,
    can_act: bool,
    config: &'a Config,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let width = config
        .buffer
        .conversation
        .member_list
        .width
        .unwrap_or(DEFAULT_WIDTH);

    let pending_count = conversation.pending_member_count();

    let invited = (pending_count > 0).then(|| {
        text(format!("· {pending_count} invited"))
            .size(theme::TEXT_SIZE - 2.0)
            .style(theme::text::tertiary)
            .font_maybe(theme::font_style::tertiary(theme).map(font::get))
    });

    let header = row![
        text("MEMBERS")
            .size(theme::TEXT_SIZE - 2.0)
            .style(theme::text::secondary)
            .font_maybe(theme::font_style::secondary(theme).map(font::get)),
        text(conversation.joined_member_count().to_string())
            .size(theme::TEXT_SIZE - 2.0)
            .style(theme::text::secondary)
            .font_maybe(theme::font_style::secondary(theme).map(font::get)),
        invited,
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);

    let members = column(
        conversation
            .members
            .iter()
            .map(|member| member_row(member, config, theme)),
    )
    .spacing(2);

    let add_member = button(
        center(
            row![
                icon::plus().size(theme::TEXT_SIZE - 2.0),
                text("Add member").style(theme::text::primary).font_maybe(
                    theme::font_style::primary(theme).map(font::get)
                ),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .height(Length::Shrink),
    )
    .width(Length::Fill)
    .padding(5)
    .style(|theme, status| theme::button::secondary(theme, status, false))
    .on_press_maybe(can_act.then_some(Message::AddMember));

    container(
        column![
            header,
            Scrollable::new(members)
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::default().width(2).scroller_width(2),
                ))
                .height(Length::Fill),
            rule::horizontal(1),
            add_member,
        ]
        .spacing(8),
    )
    .width(Length::Fixed(width))
    .height(Length::Fill)
    .padding(padding::all(4).left(8))
    .into()
}

fn member_row<'a>(
    member: &'a Member,
    config: &'a Config,
    theme: &'a Theme,
) -> Element<'a, Message> {
    let label_style = if member.pending {
        theme::text::tertiary
    } else {
        theme::text::primary
    };

    let label = text(
        member
            .address
            .as_ref()
            .map_or("unknown", Address::short_label),
    )
    .style(label_style)
    .font(font::MONO.clone());

    let you_badge = member.is_self.then(|| {
        container(
            text("you")
                .size(theme::TEXT_SIZE - 2.0)
                .style(theme::text::secondary)
                .font_maybe(theme::font_style::secondary(theme).map(font::get)),
        )
        .padding([0, 5])
        .style(theme::container::tooltip)
    });

    let pending_note = member.pending.then(|| {
        row![
            icon::spinner(0.0).width(10).height(10),
            text("Waiting to join")
                .size(theme::TEXT_SIZE - 2.0)
                .style(theme::text::tertiary)
                .font_maybe(theme::font_style::tertiary(theme).map(font::get),),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center)
    });

    let content = row![
        avatar::member(member.address.as_ref(), member.is_self, AVATAR_SIZE),
        column![
            row![label, you_badge]
                .spacing(6)
                .align_y(iced::Alignment::Center),
            pending_note,
        ]
        .spacing(1),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    let base = container(content).width(Length::Fill).padding([3, 2]);

    let Some(address) = member.address.clone() else {
        return base.into();
    };

    let base = tooltip(
        base,
        Some(address.as_str().to_string()),
        tooltip::Position::Left,
        theme,
    );

    context_menu(
        context_menu::MouseButton::default(),
        context_menu::Anchor::Cursor,
        context_menu::ToggleBehavior::KeepOpen,
        Some(mouse::Interaction::Pointer),
        base,
        Entry::list(member.is_self),
        move |entry, length| {
            let (label, message) = match entry {
                Entry::CopyAddress => (
                    "Copy address",
                    Message::CopyAddress(address.as_str().to_string()),
                ),
                Entry::Dm => (
                    "DM this member",
                    Message::Dm(address.as_str().to_string()),
                ),
            };

            button(
                text(label).style(theme::text::primary).font_maybe(
                    theme::font_style::primary(theme).map(font::get),
                ),
            )
            .width(length)
            .padding(config.context_menu.padding.entry)
            .style(|theme, status| theme::button::primary(theme, status, false))
            .on_press(message)
            .into()
        },
    )
    .into()
}
