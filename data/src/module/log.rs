//! Two-stage parser for the daemon's combined module log.
//!
//! The daemon writes one file for every module it supervises, and a line in it
//! is really two grammars stacked. The outer *envelope* is written by the
//! daemon and says when the line arrived and which module it came from; the
//! *payload* is whatever the module itself printed, in whatever logging
//! dialect that module happens to use. The three dialects we ship — Rust
//! `tracing`, nim/waku, and freeform Qt/spdlog — disagree about everything
//! including whether they carry a level at all.
//!
//! Two rules drive the whole design, both learned from a real session rather
//! than assumed:
//!
//! 1. **The payload's level wins.** Every one of blockchain's `ERROR` lines
//!    arrived under envelope `[out]`, and its fatal abort arrived under
//!    `[info]`. Colouring by the envelope alone is not merely lossy, it is
//!    wrong in exactly the cases the user is looking for.
//! 2. **Parsing is total.** The log file is shared and not opened `O_APPEND`,
//!    so torn and interleaved lines are structurally possible, and any line
//!    may carry SGR escapes. Every input returns a renderable [`Record`]; a
//!    line that matches no envelope is attributed to the previous line's
//!    module rather than dropped *when that line was plausibly continuable*,
//!    because that is what a multi-line panic and a raw backtrace look like.
//!    See [`continuable`] for why the inheritance is conditional.

use std::borrow::Cow;
use std::iter::Peekable;
use std::str::Chars;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::module::ModuleId;

/// Envelope timestamps are the daemon's local wall clock with no offset, so
/// they are naive by construction. Payload timestamps (ISO-8601 UTC for
/// `tracing`, local-offset for nim) are dropped: they were never observed to
/// differ from the envelope's by more than a millisecond, and keeping one
/// authoritative clock keeps the pane's ordering stable across dialects.
const ENVELOPE_TIMESTAMP: &str = "%Y-%m-%d %H:%M:%S%.3f";

/// The Qt bridge's own tag: `[ts] [level] [logos] …` versus the stdio form's
/// `[ts] [stream] …`. It is a corroborating signal, not the deciding one —
/// the discriminant's own vocabulary picks the form, so a renamed or dropped
/// bridge tag cannot cost a line its severity.
const QT_TAG: &str = "logos";

/// Freeform payloads carry no level, so a module abort is announced at
/// `info` and would render as routine traffic. These markers are the exact
/// phrases the daemon and a panicking Rust module emit around a crash; they
/// raise the line to [`Level::Error`] so the crash sequence stays visible
/// under any sane level filter.
const CRASH_MARKERS: &[&str] =
    &["panicked at", "thread panicked", "FATAL:", CRASH_NOTICE];

/// What a Qt envelope's severity resolves to when its token is outside the
/// known vocabulary. Deliberately not `Info`: a level we cannot read is not
/// evidence that nothing is wrong, and a freeform Qt payload carries nothing
/// to correct it with later.
const UNKNOWN_LEVEL: Level = Level::Warn;

/// The daemon's death notice for a supervised module. It carries no module
/// bracket — the daemon wrote it, not the module — but it names its subject as
/// the message's tail, which is a better attribution than any guess made from
/// the surrounding lines.
const CRASH_NOTICE: &str = "Module process crashed";

/// Which envelope form the line matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Form {
    /// `[ts] [level] [logos] [module] payload` — the Qt logging bridge. The
    /// module tag is optional: the daemon's own lines omit it.
    Qt,
    /// `[ts] [stream] [module] payload` — subprocess stdio passthrough.
    Stdio,
    /// No envelope. The daemon's startup banner, a continuation of a
    /// multi-line payload, or a torn write.
    Unenveloped,
}

/// Which stdio stream the line came off. `err` has never been observed, but
/// it is structurally reachable and is not treated as impossible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Stream {
    Out,
    Err,
}

/// The payload dialect that matched, so the UI can style targets and
/// key=value trailers consistently and tests can assert coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sublanguage {
    /// Rust `tracing`: ISO-8601 UTC, padded level, `::`-separated target.
    Tracing,
    /// nim/waku: three-letter level, local-offset timestamp, padded message.
    Nim,
    /// Qt/spdlog and everything else: no embedded level.
    Freeform,
}

/// Severity, ordered so a "at least warn" filter is a comparison.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

/// Where [`Record::level`] came from. Worth recording because the interesting
/// case — a payload contradicting its envelope — is invisible otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LevelSource {
    /// The payload dialect carried no level; the envelope supplied it.
    Envelope,
    /// The payload's own level, which overrides the envelope's.
    Payload,
    /// A freeform line whose text names a crash, raised above its envelope.
    CrashMarker,
}

/// What the envelope's second bracket turned out to be.
///
/// The two envelope forms are told apart by this token's own vocabulary rather
/// than by the presence of the `[logos]` bridge tag: keying on the tag meant a
/// Qt line whose third bracket was anything else was reparsed as stdio and its
/// severity thrown away, so a daemon that renamed the tag would lose every Qt
/// level at once and silently.
enum Discriminant {
    /// `out` or `err` — subprocess stdio passthrough.
    Stream(Stream),
    /// A severity word — the Qt logging bridge.
    Level(Level),
    /// Neither: an envelope shape we do not know. Read as Qt at
    /// [`UNKNOWN_LEVEL`] when the bridge tag corroborates it, and as stdio
    /// with no stream otherwise.
    Unknown,
}

/// One parsed log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub form: Form,
    /// The daemon's local wall clock. `None` only for unenveloped lines with
    /// no preceding line to inherit from.
    pub timestamp: Option<NaiveDateTime>,
    /// Stdio form only.
    pub stream: Option<Stream>,
    /// `None` for the daemon's own lines, which carry no module tag.
    pub module: Option<ModuleId>,
    /// What the envelope alone claimed, kept so the UI can show the
    /// disagreement and so the level-wins rule is testable.
    pub envelope_level: Level,
    /// The resolved level. This is the one to render.
    pub level: Level,
    pub level_source: LevelSource,
    pub sublanguage: Sublanguage,
    /// The `tracing` target, e.g. `logos_blockchain::storage`.
    pub target: Option<String>,
    /// The human message, with every envelope and dialect prefix stripped, so
    /// the UI can render a clean line and style the metadata separately.
    pub message: String,
}

impl Record {
    /// Whether this line says its module is dying.
    ///
    /// Deliberately keyed on the message rather than on
    /// [`LevelSource::CrashMarker`]: a dialect that carries its own level
    /// (a `tracing` `ERROR` naming the panic) resolves as
    /// [`LevelSource::Payload`], so reading the level's provenance would
    /// miss exactly the half of the abort sequence that says most. A module
    /// that panics is not recoverable — the runtime tears every service down
    /// (`logos-modules.md` §5) — so any of these phrases is enough to move the row out
    /// of "idle" and into "crashed".
    pub fn reports_crash(&self) -> bool {
        CRASH_MARKERS
            .iter()
            .any(|marker| self.message.contains(marker))
    }
}

/// Bridges the log's severity onto the one the panes render from
/// (`theme::selectable_text::log_level`).
///
/// One-to-one, deliberately. An earlier version folded `Critical` into
/// `Error`, which cost nothing in the common case and everything in the one
/// that matters: a module abort (`logos-modules.md` §5) came out of the pane wearing
/// the same word and the same colour as the routine errors above it, so the
/// line the monitor exists to surface was the hardest one to find.
impl From<Level> for crate::log::Level {
    fn from(level: Level) -> Self {
        match level {
            Level::Trace => Self::Trace,
            Level::Debug => Self::Debug,
            Level::Info => Self::Info,
            Level::Warn => Self::Warn,
            Level::Error => Self::Error,
            Level::Critical => Self::Critical,
        }
    }
}

/// What a dialect recovered from a payload, before the level-wins rule and
/// the crash escalation are applied.
struct Payload {
    sublanguage: Sublanguage,
    level: Option<Level>,
    target: Option<String>,
    message: String,
}

/// Parses one raw line with no surrounding context.
///
/// Use [`parse_continuing`] when feeding a stream: a line that matches no
/// envelope loses its module attribution here, which is correct for an
/// isolated line and wrong for the second line of a panic.
pub fn parse(line: &str) -> Record {
    parse_continuing(line, None)
}

/// Parses one raw line, inheriting attribution from the record before it when
/// the line carries no envelope of its own.
///
/// Multi-line panics, raw backtrace addresses and torn writes all arrive this
/// way, and all of them belong to the module whose line they interrupted.
pub fn parse_continuing(line: &str, previous: Option<&Record>) -> Record {
    let clean = strip_ansi(line);

    match envelope(clean.as_ref()) {
        Some(record) => record,
        None => continuation(clean.as_ref(), previous),
    }
}

/// Removes CSI (SGR and friends) and OSC escape sequences. Borrows unchanged
/// when the line has no escapes, which is the overwhelmingly common case; a
/// sequence torn off by a partial write simply consumes the rest of the line.
///
/// The introducer is peeked rather than consumed, so an ESC that begins
/// nothing we recognise costs only itself: consuming it unconditionally
/// deleted the following character of real message text.
fn strip_ansi(line: &str) -> Cow<'_, str> {
    if !line.contains('\u{1b}') {
        return Cow::Borrowed(line);
    }

    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();

    while let Some(character) = chars.next() {
        if character != '\u{1b}' {
            out.push(character);
            continue;
        }

        match chars.peek() {
            Some('[') => {
                chars.next();

                for parameter in chars.by_ref() {
                    if matches!(parameter, '\u{40}'..='\u{7e}') {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                skip_osc(&mut chars);
            }
            _ => {}
        }
    }

    Cow::Owned(out)
}

/// Consumes an OSC string up to and including its terminator, which is either
/// a BEL or the ESC `\` of ST. Split out because the ST form needs a second
/// character of lookahead, which cannot be taken while iterating.
fn skip_osc(chars: &mut Peekable<Chars<'_>>) {
    loop {
        match chars.next() {
            None | Some('\u{7}') => return,
            Some('\u{1b}') => {
                if chars.peek() == Some(&'\\') {
                    chars.next();
                }

                return;
            }
            Some(_) => {}
        }
    }
}

/// Splits a leading `[…]` group, returning its contents and the remainder with
/// the separating whitespace consumed.
///
/// Separators are treated as "one or more spaces" rather than exactly one: a
/// torn or re-indented write must not cost the line its module attribution.
fn bracket(rest: &str) -> Option<(&str, &str)> {
    let inner = rest.trim_start().strip_prefix('[')?;
    let end = inner.find(']')?;

    Some((&inner[..end], inner[end + 1..].trim_start()))
}

/// Splits the leading whitespace-delimited token off a payload, returning it
/// and the remainder with the separator consumed.
///
/// Both dialects pad their fields, and a torn write can leave any number of
/// spaces or a tab where the emitter wrote one space, so the scanners split on
/// runs of whitespace rather than on a single literal space.
fn token(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    let end = text.find(char::is_whitespace).unwrap_or(text.len());

    (end > 0).then(|| (&text[..end], text[end..].trim_start()))
}

/// Wire module names are lowercase snake_case. Shared by the envelope's module
/// bracket and by the daemon's crash notice, which names its subject in prose.
fn is_module_name(tag: &str) -> bool {
    !tag.is_empty()
        && tag.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
        })
}

/// A module tag if it looks like one.
///
/// Both envelope forms put the module in a bracket group, and both are
/// followed by payloads that may themselves open with a bracket —
/// `[LogosProviderObject] Saving token …` is a payload, not a module. Wire
/// module names are lowercase snake_case, which rejects every such payload
/// tag observed; a daemon line with no module tag at all opens with prose and
/// is rejected by the missing bracket.
fn module_tag(rest: &str) -> (Option<ModuleId>, &str) {
    match bracket(rest) {
        Some((tag, after)) if is_module_name(tag) => {
            (Some(ModuleId::from(tag)), after)
        }
        _ => (None, rest),
    }
}

/// The module named by the daemon's crash notice, which is the one line in the
/// log that states a module is dead and the one line with no module bracket to
/// say whose. Reading it out of the message is exact, where inheriting from
/// the preceding line would only be a guess.
fn crashed_module(message: &str) -> Option<ModuleId> {
    let name = message
        .trim()
        .strip_prefix(CRASH_NOTICE)?
        .trim_start_matches(':')
        .trim();

    is_module_name(name).then(|| ModuleId::from(name))
}

/// Peels the envelope, then hands the payload to the dialect scanners.
/// Returns `None` for anything that is not an envelope at all.
fn envelope(line: &str) -> Option<Record> {
    let (stamp, rest) = bracket(line)?;
    let timestamp =
        NaiveDateTime::parse_from_str(stamp, ENVELOPE_TIMESTAMP).ok()?;
    let (discriminant, rest) = bracket(rest)?;

    let bridged = matches!(bracket(rest), Some((QT_TAG, _)));

    let (form, stream, envelope_level) = match classify(discriminant) {
        Discriminant::Level(level) => (Form::Qt, None, level),
        Discriminant::Stream(stream) => {
            (Form::Stdio, Some(stream), stream_level(Some(stream)))
        }
        Discriminant::Unknown if bridged => (Form::Qt, None, UNKNOWN_LEVEL),
        Discriminant::Unknown => (Form::Stdio, None, stream_level(None)),
    };

    let rest = match (form, bracket(rest)) {
        (Form::Qt, Some((QT_TAG, after))) => after,
        _ => rest,
    };
    let (module, text) = module_tag(rest);

    let payload = payload(text);
    let (level, level_source) = resolve(&payload, envelope_level);
    let module = module.or_else(|| crashed_module(&payload.message));

    Some(Record {
        form,
        timestamp: Some(timestamp),
        stream,
        module,
        envelope_level,
        level,
        level_source,
        sublanguage: payload.sublanguage,
        target: payload.target,
        message: payload.message,
    })
}

/// A line with no envelope of its own: it belongs to whatever came before it,
/// including that line's severity, so a backtrace under a `critical` crash
/// line does not quietly drop back to `info`.
///
/// Attribution is inherited only from a [`continuable`] predecessor. The
/// timestamp is inherited unconditionally, because it is an ordering hint
/// rather than a claim about authorship: the line did arrive after that one.
fn continuation(text: &str, previous: Option<&Record>) -> Record {
    let inherited = previous.filter(|record| continuable(record));
    let envelope_level = inherited.map_or(Level::Info, |record| record.level);
    let payload = Payload {
        sublanguage: Sublanguage::Freeform,
        level: None,
        target: None,
        message: text.trim_end().to_owned(),
    };
    let (level, level_source) = resolve(&payload, envelope_level);

    Record {
        form: Form::Unenveloped,
        timestamp: previous.and_then(|record| record.timestamp),
        stream: inherited.and_then(|record| record.stream),
        module: inherited.and_then(|record| record.module.clone()),
        envelope_level,
        level,
        level_source,
        sublanguage: Sublanguage::Freeform,
        target: None,
        message: payload.message,
    }
}

/// Whether an unenveloped line may be attributed to `record`.
///
/// The log is one shared file and the modules write to it concurrently —
/// delivery outpaced blockchain by roughly 35 lines to 1, so "whatever line
/// came last" is usually the wrong module for a backtrace, and inheriting from
/// it files a panic under an unrelated module at that module's level. Only a
/// record that is itself crash context can be continued: the crash phrases are
/// the only ones observed to be followed by unenveloped output, and an
/// unenveloped line that already inherited extends the same run.
///
/// This does not resolve interleaving, which is genuinely ambiguous — it only
/// chooses `module: None` over a confident wrong answer. A multi-line payload
/// that is not a crash (a pretty-printed struct, say) therefore lands
/// unattributed, which is the failure we would rather have.
fn continuable(record: &Record) -> bool {
    (record.form == Form::Unenveloped && record.module.is_some())
        || CRASH_MARKERS
            .iter()
            .any(|marker| record.message.contains(marker))
}

/// The level-wins rule. A dialect that reports its own level is believed
/// outright — it is the module talking about itself, while the envelope only
/// says which pipe the bytes came down. Freeform payloads have nothing to
/// say, so they keep the envelope's level unless they name a crash, which the
/// envelope routinely files under `info`.
fn resolve(payload: &Payload, envelope_level: Level) -> (Level, LevelSource) {
    if let Some(level) = payload.level {
        return (level, LevelSource::Payload);
    }

    let crashed = CRASH_MARKERS
        .iter()
        .any(|marker| payload.message.contains(marker));

    if crashed && envelope_level < Level::Error {
        (Level::Error, LevelSource::CrashMarker)
    } else {
        (envelope_level, LevelSource::Envelope)
    }
}

/// Sorts the envelope's discriminant into the form it implies.
fn classify(discriminant: &str) -> Discriminant {
    match discriminant {
        "out" => Discriminant::Stream(Stream::Out),
        "err" => Discriminant::Stream(Stream::Err),
        other => {
            qt_level(other).map_or(Discriminant::Unknown, Discriminant::Level)
        }
    }
}

/// The full spdlog/Qt severity vocabulary, `None` for anything outside it.
///
/// Every token the emitter can produce is listed, including `trace`, `warn`
/// and `error`, which the session never happened to exercise: a token missing
/// from this table used to fall through a catch-all to `Info`, so a
/// daemon-reported error rendered as routine traffic. Returning `None` instead
/// means an unrecognised token stops looking like a Qt envelope at all, rather
/// than looking like a healthy one.
fn qt_level(token: &str) -> Option<Level> {
    match token {
        "trace" => Some(Level::Trace),
        "debug" => Some(Level::Debug),
        "info" => Some(Level::Info),
        "warn" | "warning" => Some(Level::Warn),
        "error" => Some(Level::Error),
        "critical" | "fatal" => Some(Level::Critical),
        _ => None,
    }
}

/// A floor, not a claim: stderr is where modules put things they consider
/// notable, but the payload almost always knows better and will override it.
fn stream_level(stream: Option<Stream>) -> Level {
    match stream {
        Some(Stream::Err) => Level::Warn,
        Some(Stream::Out) | None => Level::Info,
    }
}

fn payload(text: &str) -> Payload {
    tracing_payload(text)
        .or_else(|| nim_payload(text))
        .unwrap_or_else(|| Payload {
            sublanguage: Sublanguage::Freeform,
            level: None,
            target: None,
            message: text.trim_end().to_owned(),
        })
}

/// `2026-08-01T03:27:38.163672Z  INFO logos_blockchain::storage: <msg>`
///
/// The level is space-padded to width five, hence the whitespace-tolerant
/// field split. The target is only taken when it is a single unbroken token,
/// so a colon inside prose cannot be mistaken for one.
fn tracing_payload(text: &str) -> Option<Payload> {
    let (stamp, rest) = token(text)?;

    if !is_iso_utc(stamp) {
        return None;
    }

    let (level, rest) = token(rest)?;
    let level = match level {
        "TRACE" => Level::Trace,
        "DEBUG" => Level::Debug,
        "INFO" => Level::Info,
        "WARN" => Level::Warn,
        "ERROR" => Level::Error,
        _ => return None,
    };

    let (target, message) = match rest.find(": ") {
        Some(end) if !rest[..end].contains(' ') => {
            (Some(rest[..end].to_owned()), &rest[end + 2..])
        }
        _ => (None, rest),
    };

    Some(Payload {
        sublanguage: Sublanguage::Tracing,
        level: Some(level),
        target,
        message: message.trim_end().to_owned(),
    })
}

/// `INF 2026-07-31 21:32:59.363-06:00 <msg>`
///
/// Levels are nim-chronicles' `shortLogLevels`, which are three letters but
/// not always the first three: it emits `NOT` for notice and `FAT` for fatal.
/// Getting those two wrong cost a module abort its severity entirely, since an
/// unmatched leading token degrades the whole line to freeform and leaves it
/// on the envelope's `info` floor.
///
/// Messages are right-padded to align the `key=value` trailers; the padding
/// is left alone because it is load-bearing for readability, and only the
/// line's own trailing whitespace is trimmed. The message itself is optional
/// so that a write torn just after the timestamp still reports its level.
fn nim_payload(text: &str) -> Option<Payload> {
    let (level, rest) = token(text)?;
    let level = match level {
        "TRC" => Level::Trace,
        "DBG" => Level::Debug,
        "INF" | "NOT" => Level::Info,
        "WRN" => Level::Warn,
        "ERR" => Level::Error,
        "FAT" => Level::Critical,
        _ => return None,
    };

    let (date, rest) = token(rest)?;
    let (time, rest) = token(rest)?;

    if !is_date(date) || !time.contains(':') {
        return None;
    }

    Some(Payload {
        sublanguage: Sublanguage::Nim,
        level: Some(level),
        target: None,
        message: rest.trim_end().to_owned(),
    })
}

fn is_iso_utc(stamp: &str) -> bool {
    stamp.ends_with('Z') && stamp.contains('T') && is_date(stamp)
}

fn is_date(stamp: &str) -> bool {
    let bytes = stamp.as_bytes();

    bytes.len() >= 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[7] == b'-'
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 55 sanitized lines from a real session: the daemon's banner, both
    /// envelope forms, all three dialects, and the full crash sequence.
    const FIXTURE: &str =
        include_str!("../../../src/testkit/fixtures/module_logs.txt");

    /// Parses the fixture the way the log reader will: in order, each line
    /// able to inherit from the one before it.
    fn fixture() -> Vec<Record> {
        let mut records: Vec<Record> = Vec::new();

        for line in FIXTURE.lines() {
            let record = parse_continuing(line, records.last());
            records.push(record);
        }

        records
    }

    fn count(records: &[Record], predicate: impl Fn(&Record) -> bool) -> usize {
        records.iter().filter(|record| predicate(record)).count()
    }

    /// Every line is accounted for, and the split between envelope forms is
    /// pinned. The only lines allowed to fall through to the degraded path
    /// are the daemon's three startup banner lines, which genuinely have no
    /// envelope; anything else landing there means we started dropping the
    /// attribution off real module output.
    #[test]
    fn every_fixture_line_parses_into_a_named_form() {
        let records = fixture();

        assert_eq!(records.len(), 55);
        assert_eq!(count(&records, |r| r.form == Form::Qt), 18);
        assert_eq!(count(&records, |r| r.form == Form::Stdio), 34);

        let unenveloped = records
            .iter()
            .filter(|record| record.form == Form::Unenveloped)
            .collect::<Vec<_>>();

        assert_eq!(unenveloped.len(), 3);
        assert!(
            unenveloped
                .iter()
                .all(|record| record.message.contains("daemon")
                    || record.message.contains("Daemon")
                    || record.message.contains("client config")),
            "{unenveloped:?}",
        );
    }

    /// All three dialects are exercised by the fixture, and the counts pin
    /// which lines belong to which — a dialect scanner that starts claiming
    /// its neighbour's lines fails here.
    #[test]
    fn every_sublanguage_is_covered() {
        let records = fixture();

        assert_eq!(
            count(&records, |r| r.sublanguage == Sublanguage::Tracing),
            26
        );
        assert_eq!(count(&records, |r| r.sublanguage == Sublanguage::Nim), 8);
        assert_eq!(
            count(&records, |r| r.sublanguage == Sublanguage::Freeform),
            21
        );
    }

    /// Attribution is the whole point of the envelope: every line routed to
    /// the module that produced it, and the daemon's own untagged lines left
    /// unattributed rather than guessed at.
    #[test]
    fn lines_are_attributed_to_their_module() {
        let records = fixture();

        let named = |name: &'static str| {
            count(&records, move |record| {
                record.module.as_ref().is_some_and(|id| id.as_str() == name)
            })
        };

        assert_eq!(named("blockchain_module"), 37);
        assert_eq!(named("delivery_module"), 8);
        assert_eq!(named("chat_module"), 3);
        assert_eq!(named("capability_module"), 2);
        assert_eq!(count(&records, |r| r.module.is_none()), 5);
    }

    /// The single most important rule in the file. Blockchain's errors all
    /// arrive on stdout, which the envelope files as `info`; rendering the
    /// envelope's level would paint a peer-negotiation failure as routine
    /// chatter.
    #[test]
    fn payload_level_beats_the_envelope() {
        let record = parse(
            "[2026-07-31 21:27:39.362] [out] [blockchain_module] \
             2026-08-01T03:27:39.362323Z ERROR \
             logos_blockchain_cryptarchia_sync::libp2p::behaviour: Error while \
             processing tip request: Peer 12D3KooWSQ: Stream error",
        );

        assert_eq!(record.envelope_level, Level::Info);
        assert_eq!(record.level, Level::Error);
        assert_eq!(record.level_source, LevelSource::Payload);

        let records = fixture();

        assert_eq!(
            count(&records, |r| r.envelope_level == Level::Info
                && r.level >= Level::Error),
            10,
        );
    }

    /// The abort is announced at `info` too, and freeform payloads have no
    /// level of their own to override it with — so the crash markers have to.
    #[test]
    fn a_crash_announced_at_info_resolves_to_an_error() {
        let record = parse(
            "[2026-07-31 21:27:53.228] [info] [logos] [blockchain_module] \
             panicked at library/std/src/thread/local.rs:428:25:",
        );

        assert_eq!(record.envelope_level, Level::Info);
        assert_eq!(record.level, Level::Error);
        assert_eq!(record.level_source, LevelSource::CrashMarker);

        let fatal = parse(
            "[2026-07-31 21:27:53.229] [critical] [logos] \
             Module process crashed: blockchain_module",
        );

        assert_eq!(fatal.level, Level::Critical);
        assert_eq!(fatal.message, "Module process crashed: blockchain_module");
    }

    /// The death notice carries no module bracket — the daemon wrote it — but
    /// it is the one line that says the module is gone, so it has to reach
    /// that module's pane. The name comes out of the message, not out of
    /// whichever line happened to precede it.
    #[test]
    fn the_crash_notice_is_attributed_to_the_module_it_names() {
        let fatal = parse(
            "[2026-07-31 21:27:53.229] [critical] [logos] \
             Module process crashed: blockchain_module",
        );

        assert_eq!(
            fatal.module.as_ref().map(ModuleId::as_str),
            Some("blockchain_module"),
        );
        assert_eq!(fatal.message, "Module process crashed: blockchain_module");

        let daemon = parse(
            "[2026-07-31 21:27:28.193] [info] [logos] \
             Module loaded: blockchain_module",
        );

        assert_eq!(daemon.module, None);
    }

    #[test]
    fn tracing_payloads_yield_a_target_and_a_clean_message() {
        let record = parse(
            "[2026-07-31 21:27:38.166] [out] [blockchain_module] \
             2026-08-01T03:27:38.166480Z  INFO logos_blockchain::storage: \
             Service 'Storage' is ready.",
        );

        assert_eq!(record.form, Form::Stdio);
        assert_eq!(record.stream, Some(Stream::Out));
        assert_eq!(record.sublanguage, Sublanguage::Tracing);
        assert_eq!(record.level, Level::Info);
        assert_eq!(record.target.as_deref(), Some("logos_blockchain::storage"));
        assert_eq!(record.message, "Service 'Storage' is ready.");
        assert_eq!(
            record.timestamp.map(|stamp| stamp.to_string()).as_deref(),
            Some("2026-07-31 21:27:38.166"),
        );
    }

    /// The nim dialect puts its level first and its timestamp second, and
    /// pads messages out to align the `key=value` trailers. Only the
    /// envelope and the dialect prefix come off.
    #[test]
    fn nim_payloads_yield_a_level_and_a_clean_message() {
        let record = parse(
            "[2026-07-31 21:32:59.363] [out] [delivery_module] \
             WRN 2026-07-31 21:32:59.363-06:00 missing node key, generating \
             new set       topics=\"waku conf builder\" tid=9579420",
        );

        assert_eq!(record.sublanguage, Sublanguage::Nim);
        assert_eq!(record.level, Level::Warn);
        assert_eq!(record.level_source, LevelSource::Payload);
        assert_eq!(record.target, None);
        assert_eq!(
            record.message,
            "missing node key, generating new set       \
             topics=\"waku conf builder\" tid=9579420",
        );
    }

    /// A Qt payload may open with a bracket of its own. It is not a module
    /// tag, and the module it names in prose is not the module that logged
    /// the line — capability logs about blockchain here.
    #[test]
    fn qt_payloads_keep_their_own_bracket_tags() {
        let record = parse(
            "[2026-07-31 21:27:28.193] [info] [logos] [capability_module] \
             [LogosProviderObject] Saving token for module: \
             \"blockchain_module\"",
        );

        assert_eq!(record.form, Form::Qt);
        assert_eq!(record.stream, None);
        assert_eq!(record.sublanguage, Sublanguage::Freeform);
        assert_eq!(record.level, Level::Info);
        assert_eq!(
            record.module.as_ref().map(ModuleId::as_str),
            Some("capability_module"),
        );
        assert_eq!(
            record.message,
            "[LogosProviderObject] Saving token for module: \
             \"blockchain_module\"",
        );
    }

    /// `tracing` colours its own output, so SGR codes land mid-line, around
    /// the level and the target. Stripping happens before anything else
    /// looks at the text, so a coloured line and a plain one are the same
    /// record.
    #[test]
    fn ansi_escapes_do_not_change_the_parse() {
        let plain = "[2026-07-31 21:27:39.362] [out] [blockchain_module] \
                     2026-08-01T03:27:39.362323Z ERROR \
                     logos_blockchain_cryptarchia_sync::libp2p::behaviour: \
                     Error while processing tip request";
        let coloured = "[2026-07-31 21:27:39.362] [out] [blockchain_module] \
                        2026-08-01T03:27:39.362323Z \u{1b}[31mERROR\u{1b}[0m \
                        \u{1b}[2mlogos_blockchain_cryptarchia_sync::libp2p::\
                        behaviour\u{1b}[0m: Error while processing tip request";

        assert_eq!(parse(coloured), parse(plain));
        assert_eq!(parse(coloured).level, Level::Error);
    }

    /// The log is a shared file that is not opened `O_APPEND`, so partial and
    /// interleaved writes are structurally possible. None of them may panic,
    /// and none may vanish.
    #[test]
    fn malformed_input_still_renders() {
        let empty = parse("");

        assert_eq!(empty.form, Form::Unenveloped);
        assert_eq!(empty.message, "");

        let partial = parse("[2026-07-31 21:2");

        assert_eq!(partial.form, Form::Unenveloped);
        assert_eq!(partial.message, "[2026-07-31 21:2");

        let headerless =
            parse("[2026-07-31 21:27:28.193] [out] [blockchain_module]");

        assert_eq!(headerless.form, Form::Stdio);
        assert_eq!(
            headerless.module.as_ref().map(ModuleId::as_str),
            Some("blockchain_module"),
        );
        assert_eq!(headerless.message, "");

        let torn = parse(
            "[2026-07-31 21:27:39.362] [out] [blockchain_module] \u{1b}[31",
        );

        assert_eq!(torn.form, Form::Stdio);
        assert_eq!(torn.message, "");
    }

    /// A panic spans several lines and only the first carries an envelope.
    /// The rest inherit the module *and* the severity, so a backtrace cannot
    /// sink below the crash that produced it.
    #[test]
    fn continuations_inherit_the_previous_lines_module() {
        let crash = parse(
            "[2026-07-31 21:27:53.228] [critical] [logos] [blockchain_module] \
             FATAL: module 'blockchain_module' crashed (signal 6). Backtrace:",
        );
        let frame = parse_continuing("0x00000001048c1f10", Some(&crash));

        assert_eq!(frame.form, Form::Unenveloped);
        assert_eq!(frame.module, crash.module);
        assert_eq!(frame.timestamp, crash.timestamp);
        assert_eq!(frame.level, Level::Critical);
        assert_eq!(frame.message, "0x00000001048c1f10");

        let second = parse_continuing("0x00000001048c1f34", Some(&frame));

        assert_eq!(second.module, crash.module);
        assert_eq!(second.level, Level::Critical);
    }

    /// Delivery outran blockchain 35 lines to 1 in the observed session, so a
    /// backtrace address is far more likely to land after an unrelated
    /// delivery line than after the crash it belongs to. Inheriting there
    /// would file a panic under the wrong module at that module's level;
    /// unattributed is the honest answer.
    #[test]
    fn a_continuation_does_not_inherit_from_routine_traffic() {
        let routine = parse(
            "[2026-07-31 21:32:59.363] [out] [delivery_module] \
             DBG 2026-07-31 21:32:59.363-06:00 dialing peer \
             topics=\"waku relay\" tid=9579420",
        );

        assert_eq!(routine.level, Level::Debug);

        let frame = parse_continuing("0x00000001048c1f10", Some(&routine));

        assert_eq!(frame.form, Form::Unenveloped);
        assert_eq!(frame.module, None);
        assert_eq!(frame.stream, None);
        assert_eq!(frame.level, Level::Info);
        assert_eq!(frame.timestamp, routine.timestamp);
    }

    /// nim-chronicles' short level names are not simply the first three
    /// letters: notice is `NOT` and fatal is `FAT`. Matching invented tokens
    /// instead left a module abort as freeform text on the envelope's `info`
    /// floor — a two-step downgrade of the loudest thing delivery can say.
    #[test]
    fn every_nim_level_token_is_recognised() {
        let cases = [
            ("TRC", Level::Trace),
            ("DBG", Level::Debug),
            ("INF", Level::Info),
            ("NOT", Level::Info),
            ("WRN", Level::Warn),
            ("ERR", Level::Error),
            ("FAT", Level::Critical),
        ];

        for (token, expected) in cases {
            let record = parse(&format!(
                "[2026-07-31 21:32:59.363] [out] [delivery_module] \
                 {token} 2026-07-31 21:32:59.363-06:00 relay shutdown failed \
                 topics=\"waku node\" tid=1",
            ));

            assert_eq!(record.sublanguage, Sublanguage::Nim, "{token}");
            assert_eq!(record.level, expected, "{token}");
            assert_eq!(record.level_source, LevelSource::Payload, "{token}");
            assert_eq!(
                record.message,
                "relay shutdown failed topics=\"waku node\" tid=1",
                "{token}",
            );
        }
    }

    /// spdlog's vocabulary includes `error` and `warn`, which the corpus never
    /// exercised. Both used to hit a catch-all and render as `info`, so a
    /// daemon-reported failure looked like routine traffic — and a freeform Qt
    /// payload has no level of its own to put it back.
    #[test]
    fn every_qt_level_token_is_recognised() {
        let cases = [
            ("trace", Level::Trace),
            ("debug", Level::Debug),
            ("info", Level::Info),
            ("warn", Level::Warn),
            ("warning", Level::Warn),
            ("error", Level::Error),
            ("critical", Level::Critical),
            ("fatal", Level::Critical),
        ];

        for (token, expected) in cases {
            let record = parse(&format!(
                "[2026-07-31 21:27:28.193] [{token}] [logos] [chat_module] \
                 DeliveryModuleImpl: send failed",
            ));

            assert_eq!(record.form, Form::Qt, "{token}");
            assert_eq!(record.envelope_level, expected, "{token}");
            assert_eq!(record.level, expected, "{token}");
            assert_eq!(record.level_source, LevelSource::Envelope, "{token}");
        }
    }

    /// An unreadable severity is not a healthy one. The line still parses as
    /// Qt because the bridge tag says what it is, and it lands above the level
    /// filters that hide routine traffic rather than below them.
    #[test]
    fn an_unknown_qt_level_does_not_resolve_to_info() {
        let record = parse(
            "[2026-07-31 21:27:28.193] [notice] [logos] [chat_module] \
             DeliveryModuleImpl: send failed",
        );

        assert_eq!(record.form, Form::Qt);
        assert_eq!(record.envelope_level, Level::Warn);
        assert_eq!(record.level, Level::Warn);
        assert_eq!(
            record.module.as_ref().map(ModuleId::as_str),
            Some("chat_module"),
        );
    }

    /// The envelope forms are told apart by the discriminant's own vocabulary,
    /// so a Qt line that lost its `[logos]` bridge tag keeps its severity
    /// instead of being reparsed as stdio with the level thrown away.
    #[test]
    fn a_qt_line_without_the_bridge_tag_keeps_its_level() {
        let record = parse(
            "[2026-07-31 21:27:53.228] [warning] [blockchain_module] \
             storage backend is degraded",
        );

        assert_eq!(record.form, Form::Qt);
        assert_eq!(record.stream, None);
        assert_eq!(record.envelope_level, Level::Warn);
        assert_eq!(record.level, Level::Warn);
        assert_eq!(
            record.module.as_ref().map(ModuleId::as_str),
            Some("blockchain_module"),
        );
        assert_eq!(record.message, "storage backend is degraded");
    }

    /// `[err]` was never observed, but the design expects blockchain to be its
    /// first user via a Rust panic: envelope `warn` floor, escalated to error
    /// by the crash phrase, with the backtrace lines following it.
    #[test]
    fn a_panic_on_stderr_keeps_its_stream_and_its_module() {
        let panic = parse(
            "[2026-07-31 21:27:53.100] [err] [blockchain_module] \
             thread 'main' panicked at src/main.rs:10:5:",
        );

        assert_eq!(panic.form, Form::Stdio);
        assert_eq!(panic.stream, Some(Stream::Err));
        assert_eq!(panic.envelope_level, Level::Warn);
        assert_eq!(panic.level, Level::Error);
        assert_eq!(panic.level_source, LevelSource::CrashMarker);

        let frame = parse_continuing("stack backtrace:", Some(&panic));

        assert_eq!(frame.stream, Some(Stream::Err));
        assert_eq!(frame.module, panic.module);
        assert_eq!(frame.level, Level::Error);
    }

    /// A stderr line that says nothing alarming stays at the `warn` floor, and
    /// a payload level still overrides it in both directions.
    #[test]
    fn the_stderr_floor_is_a_floor_and_not_a_claim() {
        let bare = parse(
            "[2026-07-31 21:27:53.100] [err] [blockchain_module] \
             listening on 127.0.0.1:8080",
        );

        assert_eq!(bare.level, Level::Warn);
        assert_eq!(bare.level_source, LevelSource::Envelope);

        let informational = parse(
            "[2026-07-31 21:27:53.100] [err] [blockchain_module] \
             2026-08-01T03:27:38.163672Z  INFO logos_blockchain::storage: \
             Service 'Storage' is ready.",
        );

        assert_eq!(informational.envelope_level, Level::Warn);
        assert_eq!(informational.level, Level::Info);
        assert_eq!(informational.level_source, LevelSource::Payload);
    }

    /// Field separators are whatever whitespace survived the write. An extra
    /// space, a tab, or a message torn off after the timestamp must not cost
    /// the line its level — that is the degradation the module doc calls
    /// actively wrong.
    #[test]
    fn padded_and_torn_payloads_keep_their_level() {
        let padded = parse(
            "[2026-07-31 21:27:39.362] [out] [blockchain_module]  \
             2026-08-01T03:27:39.362323Z ERROR foo::bar: boom",
        );

        assert_eq!(padded.sublanguage, Sublanguage::Tracing);
        assert_eq!(padded.level, Level::Error);
        assert_eq!(padded.target.as_deref(), Some("foo::bar"));
        assert_eq!(padded.message, "boom");

        let tabbed = parse(
            "[2026-07-31 21:27:39.362] [out] [blockchain_module]\t\
             2026-08-01T03:27:39.362323Z\tERROR\tfoo::bar: boom",
        );

        assert_eq!(tabbed.level, Level::Error);
        assert_eq!(tabbed.target.as_deref(), Some("foo::bar"));

        let torn = parse(
            "[2026-07-31 21:32:59.363] [out] [delivery_module] \
             ERR 2026-07-31 21:32:59.363-06:00",
        );

        assert_eq!(torn.sublanguage, Sublanguage::Nim);
        assert_eq!(torn.level, Level::Error);
        assert_eq!(torn.level_source, LevelSource::Payload);
        assert_eq!(torn.message, "");
    }

    /// An escape that begins no sequence we know costs only itself. Consuming
    /// the character after it deleted message text, and mistaking an OSC
    /// introducer for one leaked the window title and a BEL into the line.
    #[test]
    fn an_unrecognised_escape_does_not_eat_the_next_character() {
        assert_eq!(parse("a\u{1b}Xb").message, "aXb");
        assert_eq!(parse("a\u{1b}]0;title\u{7}b").message, "ab");
        assert_eq!(parse("a\u{1b}]0;title\u{1b}\\b").message, "ab");
        assert_eq!(parse("a\u{1b}[31mb").message, "ab");
        assert_eq!(parse("a\u{1b}").message, "a");
    }

    /// The bridge to the render level is one-to-one. It used to fold
    /// `Critical` into `Error`, which made the abort in `logos-modules.md` §5
    /// indistinguishable from the routine errors it sits among; every other
    /// level maps to its own name and is pinned here so the fold cannot come
    /// back as a catch-all arm.
    #[test]
    fn the_render_level_keeps_a_crash_apart_from_an_error() {
        for (level, rendered) in [
            (Level::Trace, crate::log::Level::Trace),
            (Level::Debug, crate::log::Level::Debug),
            (Level::Info, crate::log::Level::Info),
            (Level::Warn, crate::log::Level::Warn),
            (Level::Error, crate::log::Level::Error),
            (Level::Critical, crate::log::Level::Critical),
        ] {
            assert_eq!(crate::log::Level::from(level), rendered);
        }

        let notice = parse(
            "[2026-07-31 21:27:53.229] [critical] [logos] \
             Module process crashed: blockchain_module",
        );

        assert!(notice.reports_crash());
        assert_eq!(
            crate::log::Level::from(notice.level),
            crate::log::Level::Critical,
        );
    }
}
