use iced::widget::text::LineHeight;
use iced::widget::{svg, text};

use crate::widget::Text;
use crate::widget::text_color_svg::{TextColorSvg, text_color_svg};
use crate::{Theme, font, theme};

pub fn dot<'a>() -> Text<'a> {
    to_text('\u{F111}')
}

pub fn error<'a>() -> Text<'a> {
    to_text('\u{E80D}')
}

pub fn cancel<'a>() -> Text<'a> {
    to_text('\u{E80F}')
}

pub fn maximize<'a>() -> Text<'a> {
    to_text('\u{E801}')
}

pub fn restore<'a>() -> Text<'a> {
    to_text('\u{E805}')
}

pub fn people<'a>() -> Text<'a> {
    to_text('\u{E804}')
}

pub fn search<'a>() -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/fontello/entypo-search.svg").as_slice(),
    ))
}

pub fn checkmark<'a>() -> Text<'a> {
    to_text('\u{E806}')
}

pub fn refresh<'a>() -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/fontello/entypo-arrows-ccw.svg").as_slice(),
    ))
}

pub fn megaphone<'a>() -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/fontello/entypo-megaphone.svg").as_slice(),
    ))
}

pub fn theme_editor<'a>() -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/fontello/entypo-palette.svg").as_slice(),
    ))
}

pub fn undo<'a>() -> Text<'a> {
    to_text('\u{E80B}')
}

pub fn copy<'a>() -> Text<'a> {
    to_text('\u{F0C5}')
}

pub fn popout<'a>() -> Text<'a> {
    to_text('\u{E80E}')
}

pub fn logs<'a>() -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/fontello/entypo-bucket.svg").as_slice(),
    ))
}

pub fn menu<'a>() -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/fontello/typicons-menu.svg").as_slice(),
    ))
}

pub fn scroll_to_bottom<'a>() -> Text<'a> {
    to_text('\u{F103}')
}

pub fn share<'a>() -> Text<'a> {
    to_text('\u{E813}')
}

pub fn mark_as_read<'a>() -> Text<'a> {
    to_text('\u{E817}')
}

pub fn config<'a>() -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/fontello/fontawesome-file-code.svg")
            .as_slice(),
    ))
}

pub fn open<'a>() -> Text<'a> {
    to_text('\u{F115}')
}

pub fn circle<'a>() -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/fontello/fontawesome-circle.svg").as_slice(),
    ))
}

pub fn quit<'a>() -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/fontello/mfg-labs-logout.svg").as_slice(),
    ))
}

pub fn plus<'a>() -> Text<'a> {
    to_text('\u{E820}')
}

pub fn chevron_down<'a>() -> Text<'a> {
    to_text('\u{0076}')
}

pub fn not_sent<'a>() -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/fontello/modern-pictograms-attention.svg")
            .as_slice(),
    ))
}

pub fn eraser<'a>() -> Text<'a> {
    to_text('\u{F12D}')
}

pub fn about<'a>() -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/fontello/entypo-info-circled.svg").as_slice(),
    ))
}

pub fn spinner<'a>(angle: f32) -> TextColorSvg<'a, Theme> {
    text_color_svg(svg::Handle::from_memory(
        include_bytes!("../assets/spinner.svg").as_slice(),
    ))
    .width(15)
    .height(15)
    .rotation(iced::Radians(angle))
}

fn to_text<'a>(unicode: char) -> Text<'a> {
    text(unicode.to_string())
        .line_height(LineHeight::Relative(1.0))
        .size(theme::ICON_SIZE)
        .font(*font::ICON)
}
