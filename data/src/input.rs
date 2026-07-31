use std::collections::HashMap;

use crate::conversation::ConvoId;
use crate::{Command, command};

const INPUT_HISTORY_LENGTH: usize = 100;

/// Parses composer input: a slash command or plain text. The only
/// validation is non-emptiness — no byte limits, no multiline batching,
/// no wire encoding (the module takes plain text).
pub fn parse(input: &str) -> Result<Parsed, Error> {
    if input.trim().is_empty() {
        return Err(Error::Empty);
    }

    match command::parse(input) {
        Ok(command) => Ok(Parsed::Command(command)),
        Err(command::Error::MissingSlash) => {
            Ok(Parsed::Text(input.to_string()))
        }
        Err(command::Error::HasDoubleSlash) => Ok(Parsed::Text(
            input.strip_prefix('/').unwrap_or(input).to_string(),
        )),
        Err(error) => Err(Error::Command(error)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Command(Command),
    Text(String),
}

#[derive(Debug, Clone)]
pub struct RawInput {
    pub convo_id: ConvoId,
    pub text: String,
}

// Draft persistence (drafts.json) is disabled behind the same seam as
// history: identity is ephemeral upstream, so drafts die with the run.
#[derive(Debug, Clone, Default)]
pub struct Storage {
    sent: HashMap<ConvoId, Vec<String>>,
    draft_messages: HashMap<ConvoId, String>,
    cursor_position: HashMap<ConvoId, (usize, usize)>,
}

impl Storage {
    pub fn get<'a>(&'a self, convo_id: &ConvoId) -> Cache<'a> {
        Cache {
            history: self
                .sent
                .get(convo_id)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            draft_message: self
                .draft_messages
                .get(convo_id)
                .map(AsRef::as_ref)
                .unwrap_or_default(),
            cursor_position: self.cursor_position.get(convo_id),
        }
    }

    pub fn record(&mut self, convo_id: &ConvoId, text: String) {
        self.draft_messages.remove(convo_id);
        let history = self.sent.entry(convo_id.clone()).or_default();
        history.insert(0, text);
        history.truncate(INPUT_HISTORY_LENGTH);
    }

    pub fn store_draft(&mut self, raw_input: RawInput) {
        if raw_input.text.is_empty() {
            self.draft_messages.remove(&raw_input.convo_id);
        } else {
            self.draft_messages
                .insert(raw_input.convo_id, raw_input.text);
        }
    }

    pub fn store_cursor_position(
        &mut self,
        convo_id: &ConvoId,
        position: (usize, usize),
    ) {
        self.cursor_position.insert(convo_id.clone(), position);
    }
}

/// Cached values for a buffers input
#[derive(Debug, Clone, Copy)]
pub struct Cache<'a> {
    pub history: &'a [String],
    pub draft_message: &'a str,
    pub cursor_position: Option<&'a (usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("input is empty")]
    Empty,
    #[error(transparent)]
    Command(#[from] command::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_commands_and_escapes() {
        assert_eq!(
            parse("hello there"),
            Ok(Parsed::Text("hello there".to_string()))
        );
        assert_eq!(
            parse("/dm deadbeef"),
            Ok(Parsed::Command(Command::Dm(Some("deadbeef".to_string()))))
        );
        assert_eq!(
            parse("//not a command"),
            Ok(Parsed::Text("/not a command".to_string()))
        );
        assert_eq!(parse(""), Err(Error::Empty));
        assert_eq!(parse("   "), Err(Error::Empty));
        assert!(matches!(parse("/bogus"), Err(Error::Command(_))));
    }

    #[test]
    fn storage_drafts_and_history() {
        let mut storage = Storage::default();
        let convo = ConvoId::from("c1");

        storage.store_draft(RawInput {
            convo_id: convo.clone(),
            text: "draft text".to_string(),
        });
        assert_eq!(storage.get(&convo).draft_message, "draft text");

        storage.record(&convo, "draft text".to_string());
        let cache = storage.get(&convo);
        assert_eq!(cache.draft_message, "");
        assert_eq!(cache.history, ["draft text".to_string()]);
    }
}
