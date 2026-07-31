use std::fmt;
use std::str::FromStr;

use chrono::format::SecondsFormat;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::Message;
use crate::message::source;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    Deserialize,
    Serialize,
)]
pub struct ReadMarker(DateTime<Utc>);

impl From<DateTime<Utc>> for ReadMarker {
    fn from(date_time: DateTime<Utc>) -> Self {
        Self(date_time)
    }
}

impl From<&Message> for ReadMarker {
    fn from(message: &Message) -> Self {
        Self::from(message.server_time)
    }
}

impl ReadMarker {
    pub fn latest(messages: &[Message]) -> Option<Self> {
        messages
            .iter()
            .rev()
            .find(|message| match message.target.source() {
                source::Source::Status(_) => false,
                // Logs are in their own buffer and this gives us backlog support there
                source::Source::Peer(_)
                | source::Source::Yourself
                | source::Source::Internal(_) => true,
            })
            .map(|message| message.server_time)
            .map(Self)
    }

    pub fn date_time(self) -> DateTime<Utc> {
        self.0
    }
}

impl FromStr for ReadMarker {
    type Err = chrono::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map(Self)
    }
}

impl fmt::Display for ReadMarker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.to_rfc3339_opts(SecondsFormat::Millis, true).fmt(f)
    }
}

pub fn latest_triggers_unread(messages: &[Message]) -> Option<DateTime<Utc>> {
    messages
        .iter()
        .rev()
        .find(|message| message.triggers_unread())
        .map(|message| message.server_time)
}
