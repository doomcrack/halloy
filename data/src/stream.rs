//! Thin re-export adapter over the Logos backend session machine. The
//! session state machine itself lives in `logos-chat`; the UI consumes it
//! as an iced subscription through this module and steers it via [`Map`].

use futures::channel::mpsc;
pub use logos_chat::session::run;
pub use logos_chat::{
    ActionError, BackendConfig, BackendError, ChatEvent, Control, Conversation,
    ConvoId, DeliveryState, Driver, GroupMember, Kind, Message, Phase, Status,
    Update,
};

/// Handle to the running backend session — the replacement for the
/// per-server `client::Map`. Holds the control sender delivered by the
/// first [`Update::Controller`] item of [`run`]'s stream.
#[derive(Debug, Default)]
pub struct Map(Option<mpsc::Sender<Control>>);

impl Map {
    pub fn set_controller(&mut self, controller: mpsc::Sender<Control>) {
        self.0 = Some(controller);
    }

    pub fn clear_controller(&mut self) {
        self.0 = None;
    }

    pub fn is_connected(&self) -> bool {
        self.0.is_some()
    }

    /// Best-effort send. The session bounds its control channel, so a full
    /// queue (single-dispatch module busy) drops the control with a log
    /// instead of blocking the UI thread.
    pub fn send(&mut self, control: Control) {
        let Some(sender) = &mut self.0 else {
            log::warn!("backend control dropped (no controller): {control:?}");
            return;
        };

        if let Err(error) = sender.try_send(control) {
            if error.is_disconnected() {
                self.0 = None;
            }

            log::warn!(
                "backend control dropped ({}): {:?}",
                if error.is_full() {
                    "queue full"
                } else {
                    "disconnected"
                },
                error.into_inner(),
            );
        }
    }

    /// Requests a clean shutdown. Returns whether the request reached the
    /// session — `false` means there is nothing to wait for.
    pub fn quit(&mut self) -> bool {
        let Some(sender) = &mut self.0 else {
            return false;
        };

        match sender.try_send(Control::Quit) {
            Ok(()) => true,
            Err(error) => {
                if error.is_disconnected() {
                    self.0 = None;
                }

                log::warn!("backend quit request dropped");
                false
            }
        }
    }
}
