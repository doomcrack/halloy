//! One app per backend state directory. The owning instance listens on a
//! local socket; a later one hands its url — or a plain [`FOCUS`] — over
//! and exits before it can spawn a second daemon. When the guard itself
//! cannot be established the app still starts, but as
//! [`Acquired::Unguarded`]: it must then leave anything another instance
//! could own alone.

pub use self::client::connect_and_send;
pub use self::server::{Acquired, FOCUS, acquire, listen};

mod client;
mod server;
