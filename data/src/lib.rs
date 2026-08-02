#![allow(clippy::large_enum_variant, clippy::too_many_arguments)]

// The chat domain lives in `logos-domain`, in its own repository under a
// permissive licence, because none of it is derived from halloy. It is
// re-exported here under the paths it always had, so the UI above never
// needs to know which side of the licence boundary a type came from.
pub use logos_domain::{address, conversation, delivery, module, session};

pub use self::address::Address;
pub use self::appearance::Theme;
pub use self::buffer::Buffer;
pub use self::command::Command;
pub use self::config::Config;
pub use self::conversation::Conversation;
pub use self::dashboard::Dashboard;
pub use self::delivery::Delivery;
pub use self::image::Image;
pub use self::message::Message;
pub use self::module::Module;
pub use self::notification::Notification;
pub use self::pane::Pane;
pub use self::preview::Preview;
pub use self::session::Session;
pub use self::shortcut::Shortcut;
pub use self::url::Url;
pub use self::version::Version;
pub use self::window::Window;

pub mod appearance;
pub mod audio;
pub mod buffer;
pub mod cache;
pub mod command;
mod compression;
pub mod config;
pub mod dashboard;
pub mod environment;
pub mod history;
pub mod image;
pub mod input;
pub mod log;
pub mod message;
pub mod notification;
pub mod pane;
pub mod preview;
pub mod serde;
pub mod shortcut;
pub mod stream;
pub mod time;
pub mod url;
pub mod version;
pub mod window;
