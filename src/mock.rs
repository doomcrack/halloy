//! Scripted mock backend: a `FakeTransport` factory emulating the daemon
//! plus chat_module so the whole app is clickable without logoscore. The
//! same session state machine runs on top — only the wire is fake.
//!
//! Behavior script:
//! - `init` succeeds, then delivery flips to `online` ~300ms later via a
//!   `delivery_state_changed` push event.
//! - Delivery blips: `error("mock delivery error")` ~10s after start,
//!   back to `online` ~5s later — the status bar's non-Online surface.
//! - `send_message` succeeds, emits `message_sent`, and schedules a
//!   scripted peer reply (`message_received`) ~800ms later — except in
//!   the Bob DM, which always fails with `success:false` so the send
//!   error path (draft restore + status bar) stays reachable.
//! - `create_conversation` / `create_group_conversation` mint an id and
//!   emit `conversation_created` with `is_outgoing = true`.
//! - `add_group_member` adds a pending roster entry and emits
//!   `members_changed`.
//! - Three conversations are seeded: two DMs and one group with a pending
//!   member (Carol), who commits ~15s in via `members_changed`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use logos_client::fake::{FakeController, FakeTransport};
use serde_json::{Value, json};

const MY_ADDRESS: &str =
    "9f8e7d6c5b4a39281706f5e4d3c2b1a09f8e7d6c5b4a39281706f5e4d3c2b1a0";
const ALICE: &str =
    "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const BOB: &str =
    "0b1c2d3e4f5a69788796a5b4c3d2e1f00b1c2d3e4f5a69788796a5b4c3d2e1f0";
const CAROL: &str =
    "cc01dd02ee03ff04aa05bb06cc07dd08ee09ff10aa11bb12cc13dd14ee15ff16";

const ONLINE_DELAY: Duration = Duration::from_millis(300);
const REPLY_DELAY: Duration = Duration::from_millis(800);
const BLIP_DELAY: Duration = Duration::from_millis(9_700);
const BLIP_RECOVERY: Duration = Duration::from_secs(5);

/// Sends into this seeded DM always fail, keeping the send-error path
/// (draft restore + status bar entry) reachable on mock data.
const SEND_FAILURE_CONVO: &str = "convo-bob";
const PENDING_COMMIT_CONVO: &str = "convo-crew";

/// Builds the `Driver::Fake` factory handed to `data::stream::run`. The
/// chat state is shared across factory calls, so a session restart talks
/// to the same mock world.
pub fn driver() -> Box<dyn FnMut() -> FakeTransport + Send> {
    let chat = Arc::new(Mutex::new(MockChat::seeded()));

    Box::new(move || {
        let (transport, controller) = FakeTransport::new();
        script(&controller, &chat);
        transport
    })
}

struct MockMessage {
    from_self: bool,
    content: String,
    timestamp_ms: i64,
    sender: Option<String>,
}

struct MockMember {
    address: String,
    pending: bool,
}

struct MockConversation {
    convo_id: String,
    kind: &'static str,
    nickname: Option<String>,
    name: Option<String>,
    description: Option<String>,
    messages: Vec<MockMessage>,
    members: Vec<MockMember>,
}

impl MockConversation {
    fn record(&self) -> Value {
        let last_activity_ms = self
            .messages
            .last()
            .map_or_else(now_ms, |message| message.timestamp_ms);
        let preview = self.messages.last().map(|message| {
            message.content.chars().take(160).collect::<String>()
        });

        let mut record = json!({
            "convo_id": self.convo_id,
            "kind": self.kind,
            "message_count": self.messages.len(),
            "last_activity_ms": last_activity_ms,
        });
        let object = record.as_object_mut().expect("record is an object");
        if let Some(nickname) = &self.nickname {
            object.insert("nickname".to_owned(), json!(nickname));
        }
        if let Some(name) = &self.name {
            object.insert("name".to_owned(), json!(name));
        }
        if let Some(description) = &self.description {
            object.insert("description".to_owned(), json!(description));
        }
        if let Some(preview) = preview {
            object.insert("preview".to_owned(), json!(preview));
        }

        record
    }

    fn message_records(&self) -> Value {
        Value::Array(
            self.messages
                .iter()
                .map(|message| {
                    let mut record = json!({
                        "from_self": message.from_self,
                        "content": message.content,
                        "timestamp_ms": message.timestamp_ms,
                    });
                    if let Some(sender) = &message.sender {
                        record
                            .as_object_mut()
                            .expect("record is an object")
                            .insert("sender".to_owned(), json!(sender));
                    }
                    record
                })
                .collect(),
        )
    }

    fn member_records(&self) -> Value {
        Value::Array(
            self.members
                .iter()
                .map(|member| {
                    json!({
                        "address": member.address,
                        "pending": member.pending,
                    })
                })
                .collect(),
        )
    }
}

struct MockChat {
    delivery_state: &'static str,
    delivery_detail: &'static str,
    next_id: u32,
    conversations: Vec<MockConversation>,
}

impl MockChat {
    fn seeded() -> Self {
        let now = now_ms();
        let dm = |id: &str,
                  peer: &str,
                  messages: Vec<MockMessage>|
         -> MockConversation {
            MockConversation {
                convo_id: id.to_owned(),
                kind: "direct",
                nickname: None,
                name: None,
                description: None,
                messages,
                members: vec![
                    MockMember {
                        address: MY_ADDRESS.to_owned(),
                        pending: false,
                    },
                    MockMember {
                        address: peer.to_owned(),
                        pending: false,
                    },
                ],
            }
        };

        Self {
            delivery_state: "initialising",
            delivery_detail: "",
            next_id: 1,
            conversations: vec![
                dm(
                    "convo-alice",
                    ALICE,
                    vec![
                        received(
                            ALICE,
                            "hey! is frigicom up yet?",
                            now - 3_600_000,
                        ),
                        sent("just booted the new backend", now - 3_500_000),
                        received(
                            ALICE,
                            "nice — send me something",
                            now - 3_400_000,
                        ),
                    ],
                ),
                dm(
                    "convo-bob",
                    BOB,
                    vec![received(
                        BOB,
                        "ping me when the mock works",
                        now - 86_400_000,
                    )],
                ),
                MockConversation {
                    convo_id: "convo-crew".to_owned(),
                    kind: "group",
                    nickname: None,
                    name: Some("Frigicom Crew".to_owned()),
                    description: Some("frigicom rewiring crew".to_owned()),
                    messages: vec![
                        received(ALICE, "welcome to the crew", now - 7_200_000),
                        sent("glad to be aboard", now - 7_100_000),
                    ],
                    members: vec![
                        MockMember {
                            address: MY_ADDRESS.to_owned(),
                            pending: false,
                        },
                        MockMember {
                            address: ALICE.to_owned(),
                            pending: false,
                        },
                        MockMember {
                            address: CAROL.to_owned(),
                            pending: true,
                        },
                    ],
                },
            ],
        }
    }

    fn get_mut(&mut self, convo_id: &str) -> Option<&mut MockConversation> {
        self.conversations
            .iter_mut()
            .find(|conversation| conversation.convo_id == convo_id)
    }

    fn mint_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}-{:04}", self.next_id);
        self.next_id += 1;
        id
    }
}

fn received(sender: &str, content: &str, timestamp_ms: i64) -> MockMessage {
    MockMessage {
        from_self: false,
        content: content.to_owned(),
        timestamp_ms,
        sender: Some(sender.to_owned()),
    }
}

fn sent(content: &str, timestamp_ms: i64) -> MockMessage {
    MockMessage {
        from_self: true,
        content: content.to_owned(),
        timestamp_ms,
        sender: None,
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Emits a `module_event` unless the transport already disconnected (a
/// delayed timer can outlive a session teardown).
fn emit(controller: &FakeController, event: &str, args: &[Value]) {
    if !controller.connected() {
        return;
    }

    let mut payload = vec![json!("chat_module"), json!(event)];
    payload.extend(args.iter().cloned());
    controller.emit("module_event", &Value::Array(payload).to_string());
}

fn script(controller: &FakeController, chat: &Arc<Mutex<MockChat>>) {
    controller.respond("getStatus", r#"{"status":"running"}"#);
    controller
        .respond("loadModule", r#"{"status":"ok","module":"chat_module"}"#);
    controller.respond("watchModuleEvents", "true");
    controller.respond("shutdown", "null");

    let chat = Arc::clone(chat);
    let emitter = controller.clone();
    controller.respond_with("callModuleMethod", move |args| {
        let args: Value = serde_json::from_str(args)
            .map_err(|_| logos_client::IpcError::DeadLink)?;
        let method = args
            .get(1)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let params = args.get(2).cloned().unwrap_or(Value::Array(vec![]));
        let param = |index: usize| -> String {
            params
                .get(index)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };

        let result = dispatch(&chat, &emitter, &method, &param);

        Ok(json!({
            "status": "ok",
            "module": "chat_module",
            "method": method,
            "result": result,
        })
        .to_string())
    });
}

fn success(value: Value) -> Value {
    json!({"success": true, "value": value, "error": null})
}

fn failure(reason: &str) -> Value {
    json!({"success": false, "value": null, "error": reason})
}

fn dispatch(
    chat: &Arc<Mutex<MockChat>>,
    emitter: &FakeController,
    method: &str,
    param: &dyn Fn(usize) -> String,
) -> Value {
    match method {
        "init" => {
            let chat = Arc::clone(chat);
            let emitter = emitter.clone();
            std::thread::spawn(move || {
                let set_delivery =
                    |state: &'static str, detail: &'static str| {
                        {
                            let mut chat = chat.lock().unwrap();
                            chat.delivery_state = state;
                            chat.delivery_detail = detail;
                        }
                        emit(
                            &emitter,
                            "delivery_state_changed",
                            &[json!(state), json!(detail)],
                        );
                    };

                std::thread::sleep(ONLINE_DELAY);
                set_delivery("online", "");

                std::thread::sleep(BLIP_DELAY);
                set_delivery("error", "mock delivery error");

                std::thread::sleep(BLIP_RECOVERY);
                set_delivery("online", "");

                let committed = {
                    let mut chat = chat.lock().unwrap();
                    chat.get_mut(PENDING_COMMIT_CONVO).is_some_and(
                        |conversation| {
                            conversation
                                .members
                                .iter_mut()
                                .filter(|member| member.pending)
                                .for_each(|member| member.pending = false);
                            true
                        },
                    )
                };

                if committed {
                    emit(
                        &emitter,
                        "members_changed",
                        &[json!(PENDING_COMMIT_CONVO)],
                    );
                }
            });
            success(Value::Null)
        }
        "shutdown" | "set_installation_name" => success(Value::Null),
        "get_address" => json!(MY_ADDRESS),
        "get_installation_name" => json!(""),
        "status" => {
            let chat = chat.lock().unwrap();
            json!({
                "convo_count": chat.conversations.len(),
                "delivery_state": chat.delivery_state,
                "detail": chat.delivery_detail,
            })
        }
        "list_conversations" => {
            let chat = chat.lock().unwrap();
            Value::Array(
                chat.conversations
                    .iter()
                    .map(MockConversation::record)
                    .collect(),
            )
        }
        "get_messages" => {
            let mut chat = chat.lock().unwrap();
            chat.get_mut(&param(0)).map_or(json!([]), |conversation| {
                conversation.message_records()
            })
        }
        "list_group_members" => {
            let mut chat = chat.lock().unwrap();
            chat.get_mut(&param(0))
                .map_or(json!([]), |conversation| conversation.member_records())
        }
        "send_message" => {
            let convo_id = param(0);
            let content = param(1);
            let timestamp_ms = now_ms();

            if convo_id == SEND_FAILURE_CONVO {
                return failure(
                    "the delivery channel rejected this message (scripted \
                     mock failure)",
                );
            }

            {
                let mut chat = chat.lock().unwrap();
                let Some(conversation) = chat.get_mut(&convo_id) else {
                    return failure("unknown conversation");
                };
                conversation.messages.push(sent(&content, timestamp_ms));
            }

            emit(
                emitter,
                "message_sent",
                &[json!(convo_id), json!(content), json!(timestamp_ms)],
            );

            let chat = Arc::clone(chat);
            let emitter = emitter.clone();
            std::thread::spawn(move || {
                std::thread::sleep(REPLY_DELAY);
                let reply = format!("echo: {content}");
                let timestamp_ms = now_ms();
                let sender = {
                    let mut chat = chat.lock().unwrap();
                    let Some(conversation) = chat.get_mut(&convo_id) else {
                        return;
                    };
                    let sender = conversation
                        .members
                        .iter()
                        .find(|member| {
                            !member.pending && member.address != MY_ADDRESS
                        })
                        .map_or_else(
                            || ALICE.to_owned(),
                            |m| m.address.clone(),
                        );
                    conversation.messages.push(received(
                        &sender,
                        &reply,
                        timestamp_ms,
                    ));
                    sender
                };
                emit(
                    &emitter,
                    "message_received",
                    &[
                        json!(convo_id),
                        json!(reply),
                        json!(timestamp_ms),
                        json!(sender),
                    ],
                );
            });

            success(Value::Null)
        }
        "create_conversation" => {
            let peer_address = param(0);
            let convo_id = {
                let mut chat = chat.lock().unwrap();
                let convo_id = chat.mint_id("convo-dm");
                chat.conversations.push(MockConversation {
                    convo_id: convo_id.clone(),
                    kind: "direct",
                    nickname: None,
                    name: None,
                    description: None,
                    messages: vec![],
                    members: vec![
                        MockMember {
                            address: MY_ADDRESS.to_owned(),
                            pending: false,
                        },
                        MockMember {
                            address: peer_address.clone(),
                            pending: false,
                        },
                    ],
                });
                convo_id
            };

            emit(
                emitter,
                "conversation_created",
                &[
                    json!(convo_id),
                    json!(true),
                    json!(data::address::short_label(&peer_address)),
                    json!("direct"),
                    json!(""),
                    json!(""),
                ],
            );

            success(json!(convo_id))
        }
        "create_group_conversation" => {
            let name = param(0);
            let desc = param(1);
            let convo_id = {
                let mut chat = chat.lock().unwrap();
                let convo_id = chat.mint_id("convo-group");
                chat.conversations.push(MockConversation {
                    convo_id: convo_id.clone(),
                    kind: "group",
                    nickname: None,
                    name: (!name.is_empty()).then(|| name.clone()),
                    description: (!desc.is_empty()).then(|| desc.clone()),
                    messages: vec![],
                    members: vec![MockMember {
                        address: MY_ADDRESS.to_owned(),
                        pending: false,
                    }],
                });
                convo_id
            };

            emit(
                emitter,
                "conversation_created",
                &[
                    json!(convo_id),
                    json!(true),
                    json!(""),
                    json!("group"),
                    json!(name),
                    json!(desc),
                ],
            );

            success(json!(convo_id))
        }
        "add_group_member" => {
            let convo_id = param(0);
            let peer_address = param(1);

            {
                let mut chat = chat.lock().unwrap();
                let Some(conversation) = chat.get_mut(&convo_id) else {
                    return failure("unknown conversation");
                };
                conversation.members.push(MockMember {
                    address: peer_address,
                    pending: true,
                });
            }

            emit(emitter, "members_changed", &[json!(convo_id)]);

            success(Value::Null)
        }
        "set_conversation_nickname" => {
            let convo_id = param(0);
            let nickname = param(1);

            {
                let mut chat = chat.lock().unwrap();
                let Some(conversation) = chat.get_mut(&convo_id) else {
                    return failure("unknown conversation");
                };
                conversation.nickname =
                    (!nickname.is_empty()).then_some(nickname);
            }

            emit(emitter, "conversation_updated", &[json!(convo_id)]);

            success(Value::Null)
        }
        "delete_conversation" => {
            let convo_id = param(0);

            {
                let mut chat = chat.lock().unwrap();
                chat.conversations
                    .retain(|conversation| conversation.convo_id != convo_id);
            }

            emit(emitter, "conversation_deleted", &[json!(convo_id)]);

            success(Value::Null)
        }
        _ => failure("unknown method"),
    }
}
