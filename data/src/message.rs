use std::borrow::Cow;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash as _, Hasher};
use std::sync::LazyLock;

use chrono::{DateTime, Local, TimeZone, Utc};
use const_format::concatcp;
use fancy_regex::{Match, Regex, RegexBuilder};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use url::Url;

pub use self::source::{Source, StatusKind};
use crate::address::Address;
use crate::conversation::ConvoId;
use crate::log::Level;
use crate::time::Posix;

pub mod source;

/// Shown as the sender when a received message carries no sender address
/// on the wire (upstream allows it; QML shows the same placeholder).
pub const UNKNOWN_SENDER: &str = "unknown";

// Based on https://github.com/dperini/regex-weburl.js (MIT)

const URL_PATH_UNRESERVED: &str = r#"\p{Letter}\p{Number}\-._~"#;

const URL_PATH_RESERVED: &str = r#":?@!&'()*+,;=\[\]"#;

static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(concatcp!(
        r#"(?i)("#,
        r#"(?:https?|wss?):\/\/"#,
        r#"[\p{Letter}\p{Number}\-@:%._+~#=]{1,256}"#,
        r#"(?:\.[\p{Letter}\p{Number}]{1,63})?"#,
        r#"\b(?:"#,
        r#"["#,
        URL_PATH_UNRESERVED,
        URL_PATH_RESERVED,
        r#"%\/#]"#,
        r#"*))"#
    ))
    .delegate_size_limit(15728640) // 1.5x default size_limit
    .build()
    .unwrap()
});

static EXCLUDED_TRAILING_CHARS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(r#"(?i)([\.,:;?!]*)$"#).build().unwrap()
});

// matching delimiters, used to match and strip trailing chars if no matching delimiter
const PAIRED_DELIMITERS: [(char, char); 3] =
    [('(', ')'), ('{', '}'), ('[', ']')];

const SYMMETRIC_DELIMITERS: [char; 2] = ['"', '\''];

// used for matching delimiter chars at the end of a word
static EXCLUDED_TRAILING_DELIMITER_CHARS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| RegexBuilder::new(r#"(?i)(["')\]}])$"#).build().unwrap());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Conversation { convo_id: ConvoId, source: Source },
    Logs { source: Source },
}

impl Target {
    pub fn source(&self) -> &Source {
        match self {
            Target::Conversation { source, .. } | Target::Logs { source } => {
                source
            }
        }
    }

    pub fn convo_id(&self) -> Option<&ConvoId> {
        match self {
            Target::Conversation { convo_id, .. } => Some(convo_id),
            Target::Logs { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Direction {
    Sent,
    Received,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub received_at: Posix,
    pub server_time: DateTime<Utc>,
    pub direction: Direction,
    pub target: Target,
    pub content: Content,
    pub hash: Hash,
    pub hidden_urls: HashSet<Url>,
}

impl Message {
    pub fn received(
        convo_id: ConvoId,
        sender: Option<Address>,
        content: String,
        timestamp_ms: i64,
    ) -> Self {
        let sender = sender.unwrap_or_else(|| Address::from(UNKNOWN_SENDER));

        Self::new(
            Target::Conversation {
                convo_id,
                source: Source::Peer(sender),
            },
            Direction::Received,
            parse_fragments(content),
            timestamp_ms,
        )
    }

    pub fn sent(convo_id: ConvoId, content: String, timestamp_ms: i64) -> Self {
        Self::new(
            Target::Conversation {
                convo_id,
                source: Source::Yourself,
            },
            Direction::Sent,
            parse_fragments(content),
            timestamp_ms,
        )
    }

    pub fn status(convo_id: ConvoId, kind: StatusKind, text: String) -> Self {
        Self::new(
            Target::Conversation {
                convo_id,
                source: Source::Status(kind),
            },
            Direction::Received,
            plain(text),
            Utc::now().timestamp_millis(),
        )
    }

    pub fn log(record: crate::log::Record) -> Self {
        let received_at = Posix::now();
        let server_time = record.timestamp;
        let target = Target::Logs {
            source: Source::Internal(source::Internal::Logs(record.level)),
        };
        let content = Content::Log(record);
        let hash = Hash::new(&server_time, &content, &received_at);

        Self {
            received_at,
            server_time,
            direction: Direction::Received,
            target,
            content,
            hash,
            hidden_urls: HashSet::default(),
        }
    }

    /// One line of a module's log, as a message on that module's history.
    ///
    /// The parsed [`Record`](crate::module::log::Record) is carried whole:
    /// level, target and message are all styled separately by the pane, and
    /// re-deriving them from the rendered text would mean parsing the same
    /// line twice — once here and once on every frame.
    ///
    /// The envelope's timestamp is the daemon's local wall clock, so it is
    /// resolved through the local zone rather than read as UTC; an ambiguous
    /// or absent stamp (a continuation line inheriting nothing) falls back to
    /// now, which keeps the row in arrival order where it belongs.
    pub fn module_log(line: crate::module::tail::Line) -> Self {
        let received_at = Posix::now();
        let server_time = line
            .record
            .timestamp
            .and_then(|timestamp| {
                Local.from_local_datetime(&timestamp).single()
            })
            .map_or_else(Utc::now, |timestamp| timestamp.with_timezone(&Utc));
        let target = Target::Logs {
            source: Source::Internal(source::Internal::Module(
                line.record.level,
            )),
        };
        let content = Content::ModuleLog(line.record);
        let hash = Hash::new(&server_time, &content, &received_at);

        Self {
            received_at,
            server_time,
            direction: Direction::Received,
            target,
            content,
            hash,
            hidden_urls: HashSet::default(),
        }
    }

    fn new(
        target: Target,
        direction: Direction,
        content: Content,
        timestamp_ms: i64,
    ) -> Self {
        let received_at = Posix::now();
        let server_time =
            DateTime::from_timestamp_millis(timestamp_ms).unwrap_or_default();
        let hash = Hash::new(&server_time, &content, &received_at);

        Self {
            received_at,
            server_time,
            direction,
            target,
            content,
            hash,
            hidden_urls: HashSet::default(),
        }
    }

    pub fn triggers_unread(&self) -> bool {
        matches!(self.direction, Direction::Received)
            && match self.target.source() {
                Source::Peer(_) => true,
                Source::Internal(source::Internal::Logs(level)) => {
                    match level {
                        // `Critical` cannot reach the app's own log — the
                        // `log` crate stops at `Error` — but if it ever did
                        // it would be the loudest thing in it.
                        Level::Critical | Level::Warn | Level::Error => true,
                        Level::Info | Level::Debug | Level::Trace => false,
                    }
                }
                // Errors only, unlike the app's own log. A relay node warns
                // constantly — delivery emitted warnings in the hundreds
                // over one session — so badging on `Warn` would leave every
                // module row permanently lit and train the badge away.
                Source::Internal(source::Internal::Module(level)) => {
                    use crate::module::log::Level;

                    match level {
                        Level::Error | Level::Critical => true,
                        Level::Warn
                        | Level::Info
                        | Level::Debug
                        | Level::Trace => false,
                    }
                }
                Source::Yourself | Source::Status(_) => false,
            }
    }

    pub fn plain(&self) -> Option<&str> {
        match &self.content {
            Content::Plain(s) => Some(s),
            Content::Fragments(_) | Content::Log(_) | Content::ModuleLog(_) => {
                None
            }
        }
    }

    pub fn text(&self) -> Cow<'_, str> {
        self.content.text()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hash(u64);

impl Hash {
    pub fn new(
        server_time: &DateTime<Utc>,
        content: &Content,
        received_at: &Posix,
    ) -> Self {
        let mut hasher = DefaultHasher::new();
        server_time.hash(&mut hasher);
        content.hash(&mut hasher);
        received_at.hash(&mut hasher);
        Self(hasher.finish())
    }
}

pub fn plain(text: String) -> Content {
    Content::Plain(text)
}

pub fn parse_fragments(text: String) -> Content {
    let fragments = parse_regex_fragments(&URL_REGEX, text, |url| {
        let text = url.to_string();
        Some(Fragment::Url(url.parse::<Url>().ok()?, text))
    });

    if fragments.len() == 1
        && let Some(Fragment::Text(text)) = fragments.first()
    {
        Content::Plain(text.clone())
    } else {
        Content::Fragments(fragments)
    }
}

fn parse_regex_fragments<'a>(
    regex: &Regex,
    text: impl Into<Cow<'a, str>>,
    mut fragment_match: impl FnMut(&str) -> Option<Fragment>,
) -> Vec<Fragment> {
    let text: Cow<'a, str> = text.into();

    let mut i = 0;
    let mut fragments = Vec::with_capacity(1);

    for re_match in regex.find_iter(&text).filter_map(Result::ok) {
        let (matching, trailing_punctuation) =
            filter_trailing_punctuation(re_match, &text);
        let (matching, trailing_delimiter) = filter_trailing_delimiter(
            matching,
            re_match.start(),
            re_match.end(),
            &text,
        );
        let (matching, leading_delimiter) = filter_leading_delimiter(matching);

        if let Some(fragment) = fragment_match(matching) {
            let mut leading_text = String::new();
            if i < re_match.start() {
                leading_text.push_str(&text[i..re_match.start()]);
            }
            if let Some(delimiter) = leading_delimiter {
                leading_text.push_str(delimiter);
            }
            if !leading_text.is_empty() {
                merge_text_fragment(&mut fragments, leading_text);
            }

            fragments.push(fragment);

            if trailing_delimiter.is_some() || trailing_punctuation.is_some() {
                let mut trailing = String::new();
                if let Some(delimiter) = trailing_delimiter {
                    trailing.push_str(delimiter);
                }
                if let Some(punctuation) = trailing_punctuation {
                    trailing.push_str(punctuation);
                }
                fragments.push(Fragment::Text(trailing));
            }

            i = re_match.end();
        }
    }

    let next_fragment_text = if i == 0 { &text } else { &text[i..] };
    merge_text_fragment(&mut fragments, next_fragment_text.to_string());

    fragments
}

fn merge_text_fragment(fragments: &mut Vec<Fragment>, text: String) {
    if text.is_empty() {
        return;
    }
    match fragments.pop() {
        Some(Fragment::Text(mut fragment_text)) => {
            fragment_text.push_str(&text);
            fragments.push(Fragment::Text(fragment_text));
        }
        fragment => {
            if let Some(fragment) = fragment {
                fragments.push(fragment);
            }
            fragments.push(Fragment::Text(text));
        }
    }
}

fn filter_trailing_punctuation<'a>(
    re_match: Match<'a>,
    text: &str,
) -> (&'a str, Option<&'a str>) {
    let matching = re_match.as_str();
    let (matching, trailing) = if let Some(Ok(trailing)) =
        EXCLUDED_TRAILING_CHARS_REGEX.find_iter(matching).next()
    {
        let trimmed_end = trailing.start();
        let is_end_of_text = re_match.end() >= text.len()
            || text[re_match.end()..].starts_with(|c: char| c.is_whitespace());

        if trimmed_end > 0 && trimmed_end < matching.len() && is_end_of_text {
            (&matching[..trimmed_end], Some(&matching[trimmed_end..]))
        } else {
            (matching, None)
        }
    } else {
        (matching, None)
    };

    (matching, trailing)
}

fn filter_trailing_delimiter<'a>(
    matching: &'a str,
    start: usize,
    end: usize,
    text: &str,
) -> (&'a str, Option<&'a str>) {
    let Some(Ok(trailing)) = EXCLUDED_TRAILING_DELIMITER_CHARS_REGEX
        .find_iter(matching)
        .next()
    else {
        return (matching, None);
    };

    let preceding_match = &text[..start];

    let mut trim_at = None;
    for (i, ch) in trailing.as_str().char_indices() {
        let preceding_trail = &matching[..trailing.start() + i];

        let filter = if SYMMETRIC_DELIMITERS.contains(&ch) {
            let unmatched_before_match =
                preceding_match.matches(ch).count() % 2 == 1;
            let unmatched_before_trail =
                preceding_trail.matches(ch).count() % 2 == 0;
            unmatched_before_match && unmatched_before_trail
        } else if let Some((open, _)) =
            PAIRED_DELIMITERS.iter().find(|(_, close)| *close == ch)
        {
            let orphaned_before_match = preceding_match.matches(*open).count()
                > preceding_match.matches(ch).count();
            let immediately_preceding = preceding_match.ends_with(*open);
            let orphaned_in_match = preceding_trail.matches(*open).count()
                <= preceding_trail.matches(ch).count();
            (orphaned_before_match || immediately_preceding)
                && orphaned_in_match
        } else {
            continue;
        };

        if filter {
            trim_at = Some(trailing.start() + i);
            break;
        }
    }

    if let Some(trimmed_end) = trim_at {
        let is_end_of_text = end >= text.len()
            || text[end..].starts_with(|c: char| c.is_whitespace())
            || matches!(
                EXCLUDED_TRAILING_CHARS_REGEX.find_iter(&text[end..]).next(),
                Some(Ok(_))
            );
        if trimmed_end > 0 && trimmed_end < matching.len() && is_end_of_text {
            (&matching[..trimmed_end], Some(&matching[trimmed_end..]))
        } else {
            (matching, None)
        }
    } else {
        (matching, None)
    }
}

fn filter_leading_delimiter(matching: &str) -> (&str, Option<&str>) {
    let Some(first_char) = matching.chars().next() else {
        return (matching, None);
    };

    if SYMMETRIC_DELIMITERS.contains(&first_char) {
        return (&matching[1..], Some(&matching[..1]));
    }

    (matching, None)
}

#[derive(Debug, Clone, Eq, Serialize, Deserialize)]
pub enum Content {
    Plain(String),
    Fragments(Vec<Fragment>),
    Log(crate::log::Record),
    /// One parsed line of a module's log. Kept as the record rather than as
    /// text so the pane can colour by level and dim the target without
    /// re-parsing.
    ModuleLog(crate::module::log::Record),
}

impl Content {
    pub fn text(&self) -> Cow<'_, str> {
        match self {
            Content::Plain(s) => s.into(),
            Content::Fragments(fragments) => {
                fragments.iter().map(Fragment::as_str).join("").into()
            }
            Content::Log(record) => (&record.message).into(),
            Content::ModuleLog(record) => (&record.message).into(),
        }
    }

    pub fn preview_text(&self) -> String {
        static NEWLINES: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\n+").unwrap());
        NEWLINES.replace_all(&self.text(), " ").into_owned()
    }
}

impl PartialEq for Content {
    fn eq(&self, other: &Self) -> bool {
        self.text() == other.text()
    }
}

impl std::hash::Hash for Content {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.text().hash(state);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Fragment {
    Text(String),
    Url(url::Url, String),
}

impl Fragment {
    pub fn url(&self) -> Option<&url::Url> {
        if let Self::Url(url, _) = self {
            Some(url)
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Fragment::Text(s) => s,
            Fragment::Url(_, s) => s,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Limit {
    Top(usize),
    Bottom(usize),
    Since(DateTime<Utc>),
    Around(usize, Hash),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fragments_detects_urls() {
        let content = parse_fragments(
            "have you seen https://halloy.chat yet?".to_string(),
        );

        let Content::Fragments(fragments) = content else {
            panic!("expected fragments");
        };

        assert_eq!(fragments.len(), 3);
        assert_eq!(fragments[0].as_str(), "have you seen ");
        assert_eq!(
            fragments[1].url().map(url::Url::as_str),
            Some("https://halloy.chat/")
        );
        assert_eq!(fragments[2].as_str(), " yet?");
    }

    #[test]
    fn parse_fragments_plain_text_stays_plain() {
        assert_eq!(
            parse_fragments("no links here".to_string()),
            Content::Plain("no links here".to_string())
        );
    }

    #[test]
    fn received_without_sender_uses_placeholder() {
        let message = Message::received(
            ConvoId::from("c1"),
            None,
            "hi".to_string(),
            1_000,
        );

        assert_eq!(
            message.target.source().peer().map(Address::as_str),
            Some(UNKNOWN_SENDER)
        );
        assert!(message.triggers_unread());
    }

    #[test]
    fn sent_and_status_do_not_trigger_unread() {
        let sent =
            Message::sent(ConvoId::from("c1"), "hello".to_string(), 1_000);
        assert!(!sent.triggers_unread());

        let status = Message::status(
            ConvoId::from("c1"),
            StatusKind::Info,
            "conversation created".to_string(),
        );
        assert!(!status.triggers_unread());
    }
}
