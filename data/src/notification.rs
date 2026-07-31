use crate::address::Address;
use crate::conversation::ConvoId;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Notification {
    Connected,
    Disconnected,
    Reconnected,
    DirectMessage {
        convo_id: ConvoId,
        sender: Address,
        message: String,
    },
    GroupMessage {
        convo_id: ConvoId,
        /// The group's display name at notify time.
        title: String,
        message: String,
    },
    /// A non-outgoing `conversation_created` — someone added us.
    GroupInvite {
        convo_id: ConvoId,
        title: String,
    },
}
