//! Wire types matching chat_module.lidl v0.2.0 exactly. The `result`-shaped
//! envelope `{"success","value","error"}` is handled in [`crate::client`];
//! these are the record payloads.

use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(transparent)]
pub struct ConvoId(pub String);

impl ConvoId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConvoId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Direct,
    Group,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Conversation {
    pub convo_id: ConvoId,
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default)]
    pub message_count: i64,
    #[serde(default)]
    pub last_activity_ms: i64,
    pub kind: Kind,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Message {
    pub from_self: bool,
    pub content: String,
    pub timestamp_ms: i64,
    /// Sender's directory-verified account address (device id when
    /// unassociated); absent on messages this installation sent.
    #[serde(default)]
    pub sender: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GroupMember {
    /// Empty string on the wire means "no confirmed account".
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeliveryState {
    #[default]
    Initialising,
    Online,
    Error,
    Stopped,
    /// Forward-compat: an unrecognized wire string. Session policy is
    /// keep-previous-state (log it), matching the QML reference client.
    Unknown,
}

impl DeliveryState {
    pub fn parse(s: &str) -> Self {
        match s {
            "initialising" => Self::Initialising,
            "online" => Self::Online,
            "error" => Self::Error,
            "stopped" => Self::Stopped,
            _ => Self::Unknown,
        }
    }

    pub fn is_online(self) -> bool {
        matches!(self, Self::Online)
    }
}

impl<'de> Deserialize<'de> for DeliveryState {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::parse(&s))
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub convo_count: i64,
    pub delivery_state: DeliveryState,
    #[serde(default)]
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_decodes_with_optional_fields_absent() {
        let json = r#"{"convo_id":"abc","kind":"direct","message_count":2,"last_activity_ms":1000}"#;
        let convo: Conversation = serde_json::from_str(json).unwrap();
        assert_eq!(convo.convo_id.as_str(), "abc");
        assert_eq!(convo.kind, Kind::Direct);
        assert_eq!(convo.nickname, None);
        assert_eq!(convo.preview, None);
    }

    #[test]
    fn delivery_state_parses_known_and_unknown() {
        assert_eq!(DeliveryState::parse("online"), DeliveryState::Online);
        assert_eq!(DeliveryState::parse("stopped"), DeliveryState::Stopped);
        assert_eq!(DeliveryState::parse("weird"), DeliveryState::Unknown);
    }

    #[test]
    fn status_decodes() {
        let json = r#"{"convo_count":3,"delivery_state":"online","detail":""}"#;
        let status: Status = serde_json::from_str(json).unwrap();
        assert!(status.delivery_state.is_online());
        assert_eq!(status.convo_count, 3);
    }
}
