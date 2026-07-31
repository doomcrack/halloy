//! Typed chat_module calls through the core_service gateway.
//!
//! Every call goes `Gateway::call_module("chat_module", method, args)`; the
//! gateway already unwrapped the `callModuleMethod` envelope, so what
//! arrives here is the module-level value. `result`-returning methods add
//! one more layer: `{"success": bool, "value": <any>, "error": <string|
//! null>}` — decoded here into `Result<Value, ChatError::Unsuccessful>`.
//! Non-`result` methods (get_address, get_installation_name,
//! list_conversations, get_messages, list_group_members, status) return
//! their value directly.

use std::time::Duration;

use logos_client::Gateway;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::ChatError;
use crate::types::{Conversation, ConvoId, GroupMember, Message, Status};

pub const MODULE: &str = "chat_module";

/// Matches the protocol's 20s ceiling; the module is single-dispatch, so a
/// slow MLS send can hold later calls for the full window.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone)]
pub struct ChatClient {
    gateway: Gateway,
}

impl ChatClient {
    pub fn new(gateway: Gateway) -> Self {
        Self { gateway }
    }

    /// `init` takes one `ChatConfig` record (chat_module 0.2.1); before that it
    /// took the preset as a bare string. The two disagree about the argument,
    /// and neither rejects the other's: a 0.2.1 module fails to parse a bare
    /// string as a record, a 0.2.0 module reads a record as an empty string,
    /// and both then fall back to their `logos.dev` default. So the preset is
    /// only honoured by a module of the matching generation — never sent to the
    /// wrong network, but silently ignored by the other one. We send the
    /// record; `logos.test` in particular needs a 0.2.1 module to take effect.
    pub async fn init(&self, delivery_preset: &str) -> Result<(), ChatError> {
        self.result_call(
            "init",
            vec![json!({ "delivery_preset": delivery_preset })],
        )
        .await?;
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), ChatError> {
        self.result_call("shutdown", Vec::new()).await?;
        Ok(())
    }

    pub async fn get_address(&self) -> Result<String, ChatError> {
        self.direct_call("get_address", Vec::new()).await
    }

    /// Returns the new conversation id (the `result.value`).
    pub async fn create_conversation(
        &self,
        peer_address: &str,
    ) -> Result<ConvoId, ChatError> {
        self.result_call("create_conversation", vec![json!(peer_address)])
            .await
            .and_then(|value| convo_id("create_conversation", &value))
    }

    pub async fn create_group_conversation(
        &self,
        name: &str,
        desc: &str,
    ) -> Result<ConvoId, ChatError> {
        self.result_call(
            "create_group_conversation",
            vec![json!(name), json!(desc)],
        )
        .await
        .and_then(|value| convo_id("create_group_conversation", &value))
    }

    pub async fn add_group_member(
        &self,
        convo_id: &ConvoId,
        peer_address: &str,
    ) -> Result<(), ChatError> {
        self.result_call(
            "add_group_member",
            vec![json!(convo_id.as_str()), json!(peer_address)],
        )
        .await?;
        Ok(())
    }

    pub async fn list_conversations(
        &self,
    ) -> Result<Vec<Conversation>, ChatError> {
        self.direct_call("list_conversations", Vec::new()).await
    }

    pub async fn get_messages(
        &self,
        convo_id: &ConvoId,
    ) -> Result<Vec<Message>, ChatError> {
        self.direct_call("get_messages", vec![json!(convo_id.as_str())])
            .await
    }

    pub async fn list_group_members(
        &self,
        convo_id: &ConvoId,
    ) -> Result<Vec<GroupMember>, ChatError> {
        self.direct_call("list_group_members", vec![json!(convo_id.as_str())])
            .await
    }

    pub async fn send_message(
        &self,
        convo_id: &ConvoId,
        content: &str,
    ) -> Result<(), ChatError> {
        self.result_call(
            "send_message",
            vec![json!(convo_id.as_str()), json!(content)],
        )
        .await?;
        Ok(())
    }

    pub async fn set_conversation_nickname(
        &self,
        convo_id: &ConvoId,
        nickname: &str,
    ) -> Result<(), ChatError> {
        self.result_call(
            "set_conversation_nickname",
            vec![json!(convo_id.as_str()), json!(nickname)],
        )
        .await?;
        Ok(())
    }

    pub async fn delete_conversation(
        &self,
        convo_id: &ConvoId,
    ) -> Result<(), ChatError> {
        self.result_call("delete_conversation", vec![json!(convo_id.as_str())])
            .await?;
        Ok(())
    }

    /// Returns the empty string while no name has been set.
    pub async fn get_installation_name(&self) -> Result<String, ChatError> {
        self.direct_call("get_installation_name", Vec::new()).await
    }

    pub async fn set_installation_name(
        &self,
        name: &str,
    ) -> Result<(), ChatError> {
        self.result_call("set_installation_name", vec![json!(name)])
            .await?;
        Ok(())
    }

    pub async fn status(&self) -> Result<Status, ChatError> {
        self.direct_call("status", Vec::new()).await
    }

    pub fn gateway(&self) -> &Gateway {
        &self.gateway
    }

    /// One `result`-envelope call: decodes `{"success","value","error"}`
    /// and returns the inner `value`.
    async fn result_call(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, ChatError> {
        let value = self
            .gateway
            .call_module(MODULE, method, args, CALL_TIMEOUT)
            .await?;
        unwrap_result(method, value)
    }

    /// One direct-value call, deserialized straight into `T`.
    async fn direct_call<T: DeserializeOwned>(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<T, ChatError> {
        let value = self
            .gateway
            .call_module(MODULE, method, args, CALL_TIMEOUT)
            .await?;
        serde_json::from_value(value)
            .map_err(|e| ChatError::Decode(format!("{method}: {e}")))
    }
}

/// Decodes the module-level `result` envelope, returning the inner `value`
/// on success and the module's error string (with a generic fallback when
/// it is null or empty) otherwise.
fn unwrap_result(method: &str, mut value: Value) -> Result<Value, ChatError> {
    match value.get("success").and_then(Value::as_bool) {
        Some(true) => {
            Ok(value.get_mut("value").map_or(Value::Null, Value::take))
        }
        Some(false) => {
            let reason = value
                .get("error")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
                .map_or_else(
                    || format!("{method} reported an unspecified failure"),
                    str::to_owned,
                );
            Err(ChatError::Unsuccessful(reason))
        }
        None => Err(ChatError::Decode(format!(
            "{method}: response is not a result envelope: {value}"
        ))),
    }
}

/// Reads a conversation id out of a `result.value`.
fn convo_id(method: &str, value: &Value) -> Result<ConvoId, ChatError> {
    value
        .as_str()
        .map(|id| ConvoId(id.to_owned()))
        .ok_or_else(|| {
            ChatError::Decode(format!(
                "{method}: value is not a conversation id: {value}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use logos_client::fake::FakeController;
    use logos_client::{FakeTransport, LogosHandle};
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::types::Kind;

    async fn client() -> (ChatClient, FakeController) {
        let (transport, controller) = FakeTransport::new();
        let (events_tx, _events_rx) = mpsc::unbounded_channel();
        let handle = LogosHandle::spawn(transport, events_tx).await.unwrap();
        (ChatClient::new(Gateway::new(Arc::new(handle))), controller)
    }

    fn envelope(result: Value) -> String {
        json!({
            "status": "ok",
            "module": "chat_module",
            "method": "x",
            "result": result,
        })
        .to_string()
    }

    #[tokio::test]
    async fn init_sends_the_preset_and_accepts_success() {
        let (client, controller) = client().await;
        controller.respond(
            "callModuleMethod",
            &envelope(json!({"success": true, "value": null, "error": null})),
        );

        client.init("logos.test").await.unwrap();

        assert_eq!(
            controller.calls(),
            vec![(
                "callModuleMethod".to_owned(),
                r#"["chat_module","init",[{"delivery_preset":"logos.test"}]]"#
                    .to_owned()
            )]
        );
    }

    #[tokio::test]
    async fn failure_envelope_maps_to_unsuccessful_with_fallback() {
        let (client, controller) = client().await;
        controller.respond_once(
            "callModuleMethod",
            Ok(envelope(json!({
                "success": false,
                "value": null,
                "error": "no delivery",
            }))),
        );
        let error = client.init("").await.unwrap_err();
        assert!(matches!(
            error,
            ChatError::Unsuccessful(reason) if reason == "no delivery"
        ));

        controller.respond_once(
            "callModuleMethod",
            Ok(envelope(
                json!({"success": false, "value": null, "error": null}),
            )),
        );
        let error = client.init("").await.unwrap_err();
        assert!(matches!(
            error,
            ChatError::Unsuccessful(reason)
                if reason == "init reported an unspecified failure"
        ));

        controller.respond_once("callModuleMethod", Ok(envelope(json!(5))));
        assert!(matches!(
            client.init("").await.unwrap_err(),
            ChatError::Decode(_)
        ));
    }

    #[tokio::test]
    async fn create_conversation_returns_the_new_convo_id() {
        let (client, controller) = client().await;
        controller.respond(
            "callModuleMethod",
            &envelope(json!({
                "success": true,
                "value": "convo-42",
                "error": null,
            })),
        );

        let convo = client.create_conversation("peer-addr").await.unwrap();

        assert_eq!(convo.as_str(), "convo-42");
        assert_eq!(
            controller.calls(),
            vec![(
                "callModuleMethod".to_owned(),
                r#"["chat_module","create_conversation",["peer-addr"]]"#
                    .to_owned()
            )]
        );

        controller.respond_once(
            "callModuleMethod",
            Ok(envelope(json!({
                "success": true,
                "value": 7,
                "error": null,
            }))),
        );
        assert!(matches!(
            client.create_conversation("peer-addr").await.unwrap_err(),
            ChatError::Decode(_)
        ));
    }

    #[tokio::test]
    async fn installation_name_round_trips() {
        let (client, controller) = client().await;
        controller.respond_once(
            "callModuleMethod",
            Ok(envelope(
                json!({"success": true, "value": null, "error": null}),
            )),
        );
        client.set_installation_name("laptop").await.unwrap();

        controller
            .respond_once("callModuleMethod", Ok(envelope(json!("laptop"))));
        assert_eq!(client.get_installation_name().await.unwrap(), "laptop");

        assert_eq!(
            controller.calls(),
            vec![
                (
                    "callModuleMethod".to_owned(),
                    r#"["chat_module","set_installation_name",["laptop"]]"#
                        .to_owned()
                ),
                (
                    "callModuleMethod".to_owned(),
                    r#"["chat_module","get_installation_name",[]]"#.to_owned()
                ),
            ]
        );
    }

    #[tokio::test]
    async fn direct_value_methods_decode_and_reject_mismatches() {
        let (client, controller) = client().await;
        controller.respond_once(
            "callModuleMethod",
            Ok(envelope(json!("my-address"))),
        );
        assert_eq!(client.get_address().await.unwrap(), "my-address");

        controller.respond_once(
            "callModuleMethod",
            Ok(envelope(json!([{
                "convo_id": "c1",
                "kind": "group",
                "name": "Team",
            }]))),
        );
        let conversations = client.list_conversations().await.unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].kind, Kind::Group);

        controller.respond_once(
            "callModuleMethod",
            Ok(envelope(json!({"delivery_state": "online"}))),
        );
        let status = client.status().await.unwrap();
        assert!(status.delivery_state.is_online());

        controller
            .respond_once("callModuleMethod", Ok(envelope(json!({"nope": 1}))));
        assert!(matches!(
            client.status().await.unwrap_err(),
            ChatError::Decode(_)
        ));
    }
}
