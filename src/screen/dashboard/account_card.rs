//! This account's card at the sidebar's foot (QML `AccountCard` parity):
//! avatar with presence, short label, delivery status, the full address
//! with a one-tap copy and a "Copied to clipboard" flash, plus the
//! overflow menu that used to hang off the user-menu button.

use std::time::Duration;

use data::config::sidebar::InternalBuffer;
use data::delivery::DeliveryState;
use data::{Config, Version, buffer, history};
use iced::widget::text::{Ellipsis, Shaping, Wrapping};
use iced::widget::{
    Space, button, center, column, container, row, rule, stack,
};
use iced::{Alignment, Border, Length, mouse};

use super::sidebar::Message;
use crate::widget::text_color_svg::TextColorSvg;
use crate::widget::{Element, Text, avatar, context_menu, text, tooltip};
use crate::{Theme, font, icon, theme};

/// How long the address field confirms a copy (QML parity).
pub const COPY_FLASH_DURATION: Duration = Duration::from_secs(2);

const AVATAR_SIZE: f32 = 28.0;

/// Shown characters at each end of the middle-elided address.
const ADDRESS_EDGE_CHARS: usize = 12;

/// Copy-flash state; generation-counted so a re-copy restarts the timer.
#[derive(Clone, Copy, Default)]
pub struct AccountCard {
    generation: u64,
    flashing: bool,
}

impl AccountCard {
    /// Starts a flash; returns the generation the expiry must match.
    pub fn copy_started(&mut self) -> u64 {
        self.generation += 1;
        self.flashing = true;
        self.generation
    }

    pub fn copy_expired(&mut self, generation: u64) {
        if generation == self.generation {
            self.flashing = false;
        }
    }
}

/// Renders the card. `width` is the sidebar's measuring-pass width
/// (`Shrink` while measuring, `Fill` once sized) — fixed `Fill`s here
/// would inflate the measured sidebar to the whole window.
pub fn view<'a>(
    account_card: AccountCard,
    session: &'a data::Session,
    history: &'a history::Manager,
    config: &'a Config,
    version: &'a Version,
    theme: &'a Theme,
    reloading_config: bool,
    system_information: Option<iced::system::Information>,
    compact: bool,
    width: Length,
) -> Element<'a, Message> {
    let online = session.delivery.can_act();
    let address = session.identity.as_ref().map(|identity| &identity.address);

    let avatar = avatar::own(address, online, AVATAR_SIZE);

    let menu = config.sidebar.user_menu.enabled.then(|| {
        overflow_menu(
            config,
            history,
            version,
            theme,
            reloading_config,
            system_information,
        )
    });

    if compact {
        return row![avatar, menu]
            .spacing(6)
            .align_y(Alignment::Center)
            .into();
    }

    let identity_column = column![
        address.map(|address| {
            text(address.short_label())
                .font(font::MONO_BOLD.clone())
                .style(theme::text::primary)
                .wrapping(Wrapping::None)
                .ellipsis(Ellipsis::End)
        }),
        tooltip(
            text(status_label(session.delivery.status))
                .size(theme::TEXT_SIZE - 1.0)
                .style(if online {
                    theme::text::success
                } else {
                    theme::text::tertiary
                })
                .font_maybe(
                    theme::font_style::secondary(theme).map(font::get),
                ),
            (!session.delivery.detail.is_empty())
                .then_some(session.delivery.detail.as_str()),
            tooltip::Position::Top,
            theme,
        ),
    ]
    .spacing(2)
    .width(width);

    let header = row![avatar, identity_column, menu]
        .spacing(8)
        .align_y(Alignment::Center);

    let address_field = address.map(|address| {
        let value: Text<'a> = if account_card.flashing {
            text("Copied to clipboard").style(theme::text::success)
        } else {
            text(middle_ellipsis(address.as_str()))
                .style(theme::text::secondary)
        };

        let copy_button = tooltip(
            button(center(icon::copy().size(10)))
                .width(20.0)
                .height(20.0)
                .padding(2)
                .style(|theme, status| {
                    theme::button::secondary(theme, status, false)
                })
                .on_press(Message::CopyAddress(address.to_string())),
            Some("Copy my address"),
            tooltip::Position::Top,
            theme,
        );

        container(
            row![
                value
                    .size(theme::TEXT_SIZE - 1.0)
                    .wrapping(Wrapping::None)
                    .ellipsis(Ellipsis::End)
                    .width(width),
                copy_button,
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([3, 4])
        .width(width)
        .style(|theme: &Theme| {
            let styles = theme.styles();

            container::Style {
                background: Some(styles.buffer.background_text_input.into()),
                border: Border {
                    radius: 4.0.into(),
                    width: 1.0,
                    color: styles.general.border,
                },
                ..Default::default()
            }
        })
    });

    container(column![header, address_field].spacing(6))
        .padding(8)
        .width(width)
        .style(|theme: &Theme| {
            let styles = theme.styles();

            container::Style {
                background: Some(styles.buffer.background.into()),
                border: Border {
                    radius: 6.0.into(),
                    width: 1.0,
                    color: styles.general.border,
                },
                ..Default::default()
            }
        })
        .into()
}

fn status_label(status: DeliveryState) -> &'static str {
    match status {
        DeliveryState::Online => "Online",
        DeliveryState::Error => "Error",
        DeliveryState::Stopped => "Stopped",
        DeliveryState::Initialising | DeliveryState::Unknown => {
            "Initialising..."
        }
    }
}

/// Middle-elided address (QML `ElideMiddle` parity): the copy button
/// still carries the full value.
fn middle_ellipsis(address: &str) -> String {
    let count = address.chars().count();

    if count <= ADDRESS_EDGE_CHARS * 2 + 1 {
        return address.to_string();
    }

    let start = address.chars().take(ADDRESS_EDGE_CHARS).collect::<String>();
    let end = address
        .chars()
        .skip(count - ADDRESS_EDGE_CHARS)
        .collect::<String>();

    format!("{start}…{end}")
}

#[derive(Debug, Clone, Copy)]
enum Menu {
    RefreshConfig,
    ConfigEditor,
    CommandBar,
    ThemeEditor,
    Logs,
    Version,
    Update,
    HorizontalRule,
    QuitApplication,
}

impl Menu {
    fn list(
        has_new_version: bool,
        internal_buffers_in_sidebar: &[InternalBuffer],
    ) -> Vec<Self> {
        let mut list = vec![Self::Version];

        if has_new_version {
            list.push(Self::Update);
        }

        list.extend([Self::HorizontalRule, Self::CommandBar]);

        if !internal_buffers_in_sidebar.contains(&InternalBuffer::Logs) {
            list.push(Self::Logs);
        }

        list.extend([
            Self::ConfigEditor,
            Self::RefreshConfig,
            Self::ThemeEditor,
            Self::QuitApplication,
        ]);

        list
    }
}

fn overflow_menu<'a>(
    config: &'a Config,
    history: &'a history::Manager,
    version: &'a Version,
    theme: &'a Theme,
    reloading_config: bool,
    system_information: Option<iced::system::Information>,
) -> Element<'a, Message> {
    let keyboard = &config.keyboard;

    let logs_has_unread = history.has_unread(&history::Kind::Logs);

    // Show notification dot if theres a new version or if the logs have
    // unread messages.
    let show_notification_dot = version.is_old()
        || (logs_has_unread
            && !config
                .sidebar
                .internal_buffers
                .buffers
                .contains(&InternalBuffer::Logs));

    let menu_icon = container(icon::menu()).width(14.0).height(14.0);

    let base: Element<'a, Message> = if show_notification_dot {
        button(stack![
            menu_icon,
            container(
                container(icon::circle().style(theme::text::tertiary))
                    .width(6.0)
                    .height(6.0),
            )
            .align_right(Length::Fill),
        ])
        .padding(4)
        .width(Length::Shrink)
        .into()
    } else {
        button(menu_icon).padding(4).width(Length::Shrink).into()
    };

    let menu =
        Menu::list(version.is_old(), &config.sidebar.internal_buffers.buffers);

    context_menu(
        context_menu::MouseButton::Left,
        context_menu::Anchor::Widget,
        context_menu::ToggleBehavior::Close,
        Some(mouse::Interaction::Pointer),
        base,
        menu,
        move |menu, length| {
            let context_button =
                |title: Text<'a>,
                 keybinds: Option<&data::shortcut::KeyBinds>,
                 icon: TextColorSvg<'a, Theme>,
                 message: Message| {
                    let title =
                        title.line_height(theme::line_height(&config.font));
                    let keybind =
                        keybinds.and_then(|key_binds| {
                            match key_binds.primary() {
                                Some(
                                    kb @ data::shortcut::KeyBind::Bind {
                                        ..
                                    },
                                ) => Some(
                                    text(format!("({kb})"))
                                        .shaping(Shaping::Advanced)
                                        .size(theme::TEXT_SIZE - 2.0)
                                        .style(theme::text::secondary)
                                        .font_maybe(
                                            theme::font_style::secondary(theme)
                                                .map(font::get),
                                        ),
                                ),
                                _ => None,
                            }
                        });

                    button(
                        row![icon.width(Length::Fixed(12.0)), title, keybind]
                            .spacing(8)
                            .align_y(iced::Alignment::Center),
                    )
                    .width(length)
                    .padding(config.context_menu.padding.entry)
                    .on_press(message)
                    .into()
                };

            match menu {
                Menu::QuitApplication => context_button(
                    text("Quit Frigicom"),
                    Some(&keyboard.quit_application),
                    icon::quit(),
                    Message::QuitApplication,
                ),
                Menu::RefreshConfig => context_button(
                    text(if reloading_config {
                        "Reloading..."
                    } else {
                        "Reload config file"
                    }),
                    Some(&keyboard.reload_configuration),
                    icon::refresh(),
                    Message::ReloadConfigFile,
                ),
                Menu::CommandBar => context_button(
                    text("Command Bar"),
                    Some(&keyboard.command_bar),
                    icon::search(),
                    Message::ToggleCommandBar,
                ),
                Menu::Logs => context_button(
                    text("Logs")
                        .style(if logs_has_unread {
                            theme::text::tertiary
                        } else {
                            theme::text::primary
                        })
                        .font_maybe(if logs_has_unread {
                            theme::font_style::tertiary(theme).map(font::get)
                        } else {
                            theme::font_style::primary(theme).map(font::get)
                        }),
                    Some(&keyboard.logs),
                    icon::logs().style(if logs_has_unread {
                        theme::text::tertiary
                    } else {
                        theme::text::primary
                    }),
                    Message::Replace(buffer::Internal::Logs.into()),
                ),
                Menu::ThemeEditor => context_button(
                    text("Theme Editor"),
                    Some(&keyboard.theme_editor),
                    icon::theme_editor(),
                    Message::ToggleThemeEditor,
                ),
                Menu::HorizontalRule => match length {
                    Length::Fill => {
                        container(rule::horizontal(1)).padding([0, 6]).into()
                    }
                    _ => Space::new().width(length).height(1).into(),
                },
                Menu::Update => context_button(
                    text("New version available")
                        .style(theme::text::tertiary)
                        .font_maybe(
                            theme::font_style::tertiary(theme).map(font::get),
                        ),
                    None,
                    icon::megaphone().style(theme::text::tertiary),
                    Message::OpenReleaseWebsite,
                ),
                Menu::Version => context_button(
                    text("About Frigicom"),
                    None,
                    icon::about(),
                    Message::OpenAbout {
                        version: version.current.clone(),
                        commit: data::environment::GIT_HASH
                            .map(str::trim)
                            .filter(|hash| !hash.is_empty())
                            .unwrap_or("Unknown")
                            .to_string(),
                        system_information: system_information.clone(),
                    },
                ),
                Menu::ConfigEditor => context_button(
                    text("Config Editor"),
                    Some(&keyboard.open_config_editor),
                    icon::config(),
                    Message::Replace(buffer::Internal::ConfigEditor.into()),
                ),
            }
        },
    )
    .into()
}
