//! The seven chat_module push events, decoded from positional JSON arrays
//! (argument order is the .lidl declaration order).

use serde_json::Value;

use crate::types::{ConvoId, DeliveryState, Kind};

#[derive(Debug, Clone, PartialEq)]
pub enum ChatEvent {
    MessageReceived {
        convo_id: ConvoId,
        content: String,
        timestamp_ms: i64,
        sender: String,
    },
    MessageSent {
        convo_id: ConvoId,
        content: String,
        timestamp_ms: i64,
    },
    ConversationCreated {
        convo_id: ConvoId,
        is_outgoing: bool,
        peer_label: String,
        kind: Kind,
        name: String,
        desc: String,
    },
    ConversationUpdated {
        convo_id: ConvoId,
    },
    MembersChanged {
        convo_id: ConvoId,
    },
    ConversationDeleted {
        convo_id: ConvoId,
    },
    DeliveryStateChanged {
        state: DeliveryState,
        detail: String,
    },
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("could not decode chat event {event}: {reason}")]
pub struct DecodeError {
    pub event: String,
    pub reason: String,
}

impl ChatEvent {
    /// Every event the session must watch, in .lidl declaration order.
    pub const NAMES: [&'static str; 7] = [
        "message_received",
        "message_sent",
        "conversation_created",
        "conversation_updated",
        "members_changed",
        "conversation_deleted",
        "delivery_state_changed",
    ];

    pub fn decode(event: &str, args: &[Value]) -> Result<Self, DecodeError> {
        let err = |reason: &str| DecodeError {
            event: event.to_owned(),
            reason: reason.to_owned(),
        };
        let string = |i: usize| -> Result<String, DecodeError> {
            args.get(i)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| err(&format!("missing string arg {i}")))
        };
        let boolean = |i: usize| -> Result<bool, DecodeError> {
            args.get(i)
                .and_then(Value::as_bool)
                .ok_or_else(|| err(&format!("missing bool arg {i}")))
        };
        let int = |i: usize| -> Result<i64, DecodeError> {
            args.get(i)
                .and_then(Value::as_i64)
                .ok_or_else(|| err(&format!("missing int arg {i}")))
        };
        let convo = |i: usize| -> Result<ConvoId, DecodeError> {
            string(i).map(ConvoId)
        };

        match event {
            "message_received" => Ok(Self::MessageReceived {
                convo_id: convo(0)?,
                content: string(1)?,
                timestamp_ms: int(2)?,
                sender: string(3)?,
            }),
            "message_sent" => Ok(Self::MessageSent {
                convo_id: convo(0)?,
                content: string(1)?,
                timestamp_ms: int(2)?,
            }),
            "conversation_created" => Ok(Self::ConversationCreated {
                convo_id: convo(0)?,
                is_outgoing: boolean(1)?,
                peer_label: string(2)?,
                kind: match string(3)?.as_str() {
                    "group" => Kind::Group,
                    _ => Kind::Direct,
                },
                name: string(4)?,
                desc: string(5)?,
            }),
            "conversation_updated" => Ok(Self::ConversationUpdated {
                convo_id: convo(0)?,
            }),
            "members_changed" => Ok(Self::MembersChanged {
                convo_id: convo(0)?,
            }),
            "conversation_deleted" => Ok(Self::ConversationDeleted {
                convo_id: convo(0)?,
            }),
            "delivery_state_changed" => Ok(Self::DeliveryStateChanged {
                state: DeliveryState::parse(&string(0)?),
                detail: string(1)?,
            }),
            other => Err(DecodeError {
                event: other.to_owned(),
                reason: "unknown event".to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn decodes_message_received() {
        let args = [json!("c1"), json!("hello"), json!(1234), json!("addr")];
        let event = ChatEvent::decode("message_received", &args).unwrap();
        assert_eq!(
            event,
            ChatEvent::MessageReceived {
                convo_id: ConvoId("c1".into()),
                content: "hello".into(),
                timestamp_ms: 1234,
                sender: "addr".into(),
            }
        );
    }

    #[test]
    fn decodes_conversation_created() {
        let args = [
            json!("c2"),
            json!(false),
            json!("peer"),
            json!("group"),
            json!("Team"),
            json!("A group"),
        ];
        let event = ChatEvent::decode("conversation_created", &args).unwrap();
        let ChatEvent::ConversationCreated {
            is_outgoing, kind, ..
        } = event
        else {
            panic!("wrong variant");
        };
        assert!(!is_outgoing);
        assert_eq!(kind, Kind::Group);
    }

    #[test]
    fn decodes_delivery_state_changed() {
        let args = [json!("online"), json!("")];
        let event = ChatEvent::decode("delivery_state_changed", &args).unwrap();
        assert_eq!(
            event,
            ChatEvent::DeliveryStateChanged {
                state: DeliveryState::Online,
                detail: String::new(),
            }
        );
    }

    #[test]
    fn rejects_unknown_event_and_bad_arity() {
        assert!(ChatEvent::decode("no_such_event", &[]).is_err());
        assert!(
            ChatEvent::decode("message_received", &[json!("only-one")])
                .is_err()
        );
    }
}
