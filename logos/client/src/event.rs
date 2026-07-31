//! Decoding of the core_service `module_event` relay payload: a flat
//! positional JSON array `[module, event, ...args]`.

use serde_json::Value;

use crate::error::IpcError;
use crate::transport::RawEvent;

#[derive(Debug, Clone)]
pub struct ModuleEvent {
    pub module: String,
    pub event: String,
    pub args: Vec<Value>,
}

pub fn decode_module_event(raw: &RawEvent) -> Result<ModuleEvent, IpcError> {
    let value: Value =
        serde_json::from_str(&raw.payload_json).map_err(|e| {
            IpcError::Decode(format!("module_event payload is not JSON: {e}"))
        })?;
    let Value::Array(items) = value else {
        return Err(IpcError::Decode(format!(
            "module_event payload is not an array: {}",
            raw.payload_json
        )));
    };
    let mut items = items.into_iter();
    let module = items
        .next()
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| {
            IpcError::Decode("module_event missing module slot".to_owned())
        })?;
    let event = items
        .next()
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| {
            IpcError::Decode("module_event missing event slot".to_owned())
        })?;
    Ok(ModuleEvent {
        module,
        event,
        args: items.collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_flat_positional_payload() {
        let raw = RawEvent {
            name: "module_event".to_owned(),
            payload_json:
                r#"["chat_module","message_received","abc","hi",123,"sender"]"#
                    .to_owned(),
        };
        let event = decode_module_event(&raw).unwrap();
        assert_eq!(event.module, "chat_module");
        assert_eq!(event.event, "message_received");
        assert_eq!(event.args.len(), 4);
    }

    #[test]
    fn rejects_short_payload() {
        let raw = RawEvent {
            name: "module_event".to_owned(),
            payload_json: r#"["chat_module"]"#.to_owned(),
        };
        assert!(decode_module_event(&raw).is_err());
    }
}
