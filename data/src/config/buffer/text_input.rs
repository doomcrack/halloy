use serde::Deserialize;

use crate::serde::deserialize_usize_positive_integer;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TextInput {
    pub visibility: Visibility,
    pub auto_format: AutoFormat,
    pub autocomplete: Autocomplete,
    pub nickname: Nickname,
    pub key_bindings: KeyBindings,
    pub kill_to_clipboard: bool,
    #[serde(deserialize_with = "deserialize_usize_positive_integer")]
    pub max_lines: usize,
    pub send_line_delay: u64,
    pub persist: bool,
}

impl Default for TextInput {
    fn default() -> Self {
        Self {
            visibility: Visibility::default(),
            auto_format: AutoFormat::default(),
            autocomplete: Autocomplete::default(),
            nickname: Nickname::default(),
            key_bindings: KeyBindings::default(),
            kill_to_clipboard: true,
            max_lines: 5,
            send_line_delay: 100,
            persist: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyBindings {
    Default,
    Emacs,
}

impl Default for KeyBindings {
    fn default() -> Self {
        if cfg!(target_os = "macos") {
            KeyBindings::Emacs
        } else {
            KeyBindings::Default
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    Focused,
    #[default]
    Always,
}

/// Whether the composer shows your own identity label beside the input.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Nickname {
    pub enabled: bool,
}

impl Default for Nickname {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoFormat {
    #[default]
    Disabled,
    Markdown,
    All,
    #[serde(skip)]
    ForceDisabled,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Autocomplete {
    pub order_by: OrderBy,
    pub sort_direction: SortDirection,
    pub completion_suffixes: [String; 2],
}

impl Default for Autocomplete {
    fn default() -> Self {
        Self {
            order_by: OrderBy::default(),
            sort_direction: SortDirection::default(),
            completion_suffixes: [": ".to_string(), " ".to_string()],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrderBy {
    Alpha,
    #[default]
    Recent,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}
