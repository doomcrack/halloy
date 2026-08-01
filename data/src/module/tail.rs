//! Follows the daemon's combined log file and turns it into batches of
//! attributed, parsed lines.
//!
//! There is exactly one log for the whole daemon — its own Qt lines plus every
//! module's stdout and stderr, interleaved (see `logos_daemon::log_path`). The
//! supervisor owns the write handle and truncates the file with
//! `File::create` on every spawn, so a reader has to survive the file not
//! existing yet, appearing under it, and resetting to zero length mid-session.
//!
//! Nothing here is a `Subscription`: this layer is pure enough to be driven a
//! tick at a time from a test, and [`follow`] wraps it in a stream for the app.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Local;
use futures::Stream;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::module::ModuleId;
use crate::module::log::{self, Form, Level, LevelSource, Record, Sublanguage};

/// How long the tailer waits between reads. Doubles as the batch debounce:
/// the delivery module alone emitted ~1976 lines in a few minutes, and one
/// application message per line would put the UI's update loop in the path of
/// a relay node's DEBUG traffic. Four batches a second is the same rate
/// `logger.rs` settled on for the app's own log pane.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Ceiling on one batch. A tick that hits it does not seek past the
/// remainder — the rest is read on the next tick, so a burst is spread over
/// several frames instead of arriving as one multi-thousand-line message.
pub const MAX_LINES_PER_BATCH: usize = 500;

/// How far back a cold start reads, so a pane opened against an
/// already-running daemon shows recent context instead of an empty view.
const BACKFILL_BYTES: u64 = 256 * 1024;

/// Ceiling on one read. Bounds the allocation a single tick can make when the
/// file has run far ahead of us.
const READ_CHUNK_BYTES: u64 = 256 * 1024;

/// How far behind the tailer may fall before it stops trying to catch up.
/// Sleep or suspend can leave hours of daemon output unread; replaying it
/// line by line would be minutes of pointless work for output nobody will
/// scroll back to. Past this, skip forward and say so.
const MAX_BACKLOG_BYTES: u64 = 4 * 1024 * 1024;

/// How much of the file's head is kept as a restart fingerprint. The
/// supervisor truncates in place, so a respawn that outruns the reader shows
/// neither a new inode nor a shorter file — only different bytes at the
/// front. The daemon's first line names its pid and instance id, well inside
/// this window, so two runs never fingerprint alike.
const HEAD_BYTES: u64 = 256;

/// How much of the already-read region is kept as a second fingerprint,
/// checked at the byte before the read offset.
///
/// The head alone is not enough: a respawn that reproduces the first
/// [`HEAD_BYTES`] bytes and outruns the reader shows no new inode, no shorter
/// file and no changed head, so the only remaining evidence is that the bytes
/// under the offset are not the bytes we read there. Costs one small read per
/// tick, which is the same price the head fingerprint already pays.
const TAIL_BYTES: usize = 256;

/// How many bytes of newline-free output the tailer will hold before it gives
/// up on ever seeing the newline.
///
/// Without this the buffer grows at the daemon's write rate for as long as
/// the output lacks a newline — carriage-return progress output, or a torn
/// write that clobbers one — and every tick returns an empty batch while the
/// resident set climbs. No line in the corpus comes near a kilobyte, so this
/// is roughly two orders of magnitude of headroom over any real line, and it
/// bounds what the tailer holds to this plus one [`READ_CHUNK_BYTES`] read.
const MAX_PENDING_BYTES: usize = 64 * 1024;

const RESTART_MARKER: &str = "--- daemon log restarted ---";

/// Appended to the fragment emitted when [`MAX_PENDING_BYTES`] is reached, so
/// the pane says why a line ends where it does instead of just looking odd.
const TRUNCATION_MARKER: &str = " --- line truncated, resyncing ---";

/// One parsed line together with the module it belongs to.
///
/// Attribution is resolved here rather than left as [`Record::module`]'s
/// `Option`: every line has an owner once the daemon's own untagged lines are
/// routed to the `logoscore` pseudo-module, and a downstream keyed on
/// `Option<ModuleId>` would have to re-decide that on every read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub module: ModuleId,
    pub record: Record,
}

impl Line {
    pub fn level(&self) -> Level {
        self.record.level
    }

    pub fn message(&self) -> &str {
        &self.record.message
    }
}

/// What makes the file we are reading the same file we read last tick.
///
/// Truncation in place keeps the inode and resets the length; a fresh
/// instance dir replaces the file entirely. Neither is detectable from length
/// alone, and both mean "start over" — going deaf instead is the failure mode
/// worth spending a `stat` per tick to avoid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    created: Option<std::time::SystemTime>,
}

/// The file as the tailer last saw it: which file it is, how long it was, and
/// what its first [`HEAD_BYTES`] bytes were. The fingerprint is the
/// load-bearing half — truncation in place changes neither the inode nor,
/// once the daemon has written enough, the length.
#[derive(Debug, Clone)]
struct Source {
    identity: Identity,
    head: Vec<u8>,
    len: u64,
}

/// Reader-side state for one log file.
#[derive(Debug)]
pub struct Tailer {
    path: PathBuf,
    /// Byte offset of the next unread byte. Bytes held in `pending` count as
    /// read: a partial line is consumed from the file and completed in
    /// memory, never re-read.
    offset: u64,
    /// The file this offset refers to. `None` before the first successful
    /// read, and again whenever the file goes away.
    source: Option<Source>,
    /// A trailing line with no newline yet. The daemon's writers share one
    /// file description without `O_APPEND`, so a read can land mid-line at
    /// any time; emitting that would hand the parser a torn envelope.
    ///
    /// Bytes, not text: a multi-byte character split across a read is the
    /// same torn write, and decoding each piece on its own would replace both
    /// halves with `U+FFFD` even though both arrived. Decoding happens once,
    /// on the assembled line.
    pending: Vec<u8>,
    /// The last [`TAIL_BYTES`] bytes the tailer accounted for, ending exactly
    /// at `offset`. Re-read each tick and compared: see [`Tailer::restarted`].
    consumed_tail: Vec<u8>,
    /// The last emitted record, for the continuation rule: a line with no
    /// envelope of its own belongs to whatever preceded it, across tick and
    /// batch boundaries as much as within one.
    previous: Option<Record>,
    /// Set after a seek that lands mid-line. Everything up to the next
    /// newline is dropped rather than parsed as a line that begins nowhere.
    discard_partial: bool,
}

impl Tailer {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: 0,
            source: None,
            pending: Vec::new(),
            consumed_tail: Vec::new(),
            previous: None,
            discard_partial: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads whatever arrived since the last call, at most
    /// [`MAX_LINES_PER_BATCH`] lines.
    ///
    /// An empty result is the normal idle case and carries no information: a
    /// missing file, an unreadable one, and a quiet daemon are deliberately
    /// indistinguishable here. The tailer outlives all three.
    pub async fn poll(&mut self) -> Vec<Line> {
        let Ok(metadata) = tokio::fs::metadata(&self.path).await else {
            // Not created yet (the tailer starts before the first daemon
            // does) or removed under us. Forget the offset so the file is
            // read from the start when it comes back.
            self.forget();
            return Vec::new();
        };

        let Ok(head) = read_at(&self.path, 0, HEAD_BYTES).await else {
            return Vec::new();
        };

        let current = Source {
            identity: identity(&metadata),
            head,
            len: metadata.len(),
        };
        let restarted = match &self.source {
            Some(previous) => self.restarted(previous, &current).await,
            None => false,
        };
        let mut batch = Vec::new();

        if self.source.replace(current).is_none() {
            self.begin(metadata.len()).await;
        } else if restarted {
            self.restart();
            batch.push(self.synthetic(Level::Info, RESTART_MARKER.into()));
        }

        if let Some(skipped) = self.skip_backlog(metadata.len()) {
            batch.push(self.synthetic(
                Level::Warn,
                format!("--- skipped {skipped} bytes of daemon log ---"),
            ));
        }

        let Ok(chunk) =
            read_at(&self.path, self.offset, READ_CHUNK_BYTES).await
        else {
            return batch;
        };

        batch.extend(self.consume(&chunk));
        batch
    }

    /// Whether the file under us is no longer the one the offset describes.
    ///
    /// Five symptoms, because no one of them catches every case: a replaced
    /// file changes the inode but need not shrink; an in-place truncation
    /// shrinks but keeps the inode; a truncation the reader only notices
    /// after the daemon has written past the old offset does neither, and is
    /// the common case for a daemon restarted while the app is up. That last
    /// one leaves only content as evidence, and the head is not enough on its
    /// own — a new run repeats whatever banner the old one opened with — so
    /// the bytes under the offset are checked too.
    ///
    /// One case survives all five: a respawn that reproduces every byte the
    /// tailer had already read. It is undetectable by construction, and its
    /// cost is bounded to the missing marker — the bytes skipped are the same
    /// bytes already shown, so the pane's contents still match the file.
    async fn restarted(&self, previous: &Source, current: &Source) -> bool {
        previous.identity != current.identity
            || current.len < previous.len
            || current.len < self.offset
            || !current.head.starts_with(&previous.head)
            || !self.tail_matches().await
    }

    /// Whether the bytes ending at `offset` are still the bytes the tailer
    /// accounted for there. Unknown (no fingerprint yet, or an unreadable
    /// file) counts as a match: a spurious restart re-reads and re-emits the
    /// whole backfill, which is worse than a late one.
    async fn tail_matches(&self) -> bool {
        if self.consumed_tail.is_empty() {
            return true;
        }

        let window = self.consumed_tail.len() as u64;
        let Ok(bytes) = read_at(&self.path, self.offset - window, window).await
        else {
            return true;
        };

        bytes == self.consumed_tail
    }

    /// First sight of a file. Starts [`BACKFILL_BYTES`] back rather than at
    /// byte zero: the daemon may have been running for hours, and replaying
    /// its whole log to fill a pane nobody has scrolled is work with no
    /// reader.
    async fn begin(&mut self, len: u64) {
        self.offset = len.saturating_sub(BACKFILL_BYTES);
        self.discard_partial =
            self.offset > 0 && !self.follows_a_newline().await;
        self.pending.clear();
        self.consumed_tail.clear();
        self.previous = None;
    }

    /// Whether the byte before `offset` is a newline, i.e. the seek landed on
    /// a line boundary and the line that starts there is whole. Discarding it
    /// as a fragment would silently lose one complete line per cold start
    /// whose backfill offset happens to be newline-aligned.
    async fn follows_a_newline(&self) -> bool {
        read_at(&self.path, self.offset - 1, 1)
            .await
            .is_ok_and(|byte| byte.first() == Some(&b'\n'))
    }

    fn restart(&mut self) {
        self.offset = 0;
        self.discard_partial = false;
        self.pending.clear();
        self.consumed_tail.clear();
        self.previous = None;
    }

    fn forget(&mut self) {
        self.source = None;
        self.offset = 0;
        self.discard_partial = false;
        self.pending.clear();
        self.consumed_tail.clear();
        self.previous = None;
    }

    /// Jumps forward when the unread tail is larger than the tailer will ever
    /// usefully catch up on, returning the number of bytes given up.
    fn skip_backlog(&mut self, len: u64) -> Option<u64> {
        let backlog = len.saturating_sub(self.offset);

        if backlog <= MAX_BACKLOG_BYTES {
            return None;
        }

        let target = len - MAX_BACKLOG_BYTES;
        let skipped = target - self.offset;

        self.offset = target;
        self.discard_partial = true;
        self.pending.clear();
        self.consumed_tail.clear();
        self.previous = None;

        Some(skipped)
    }

    /// Splits a read into complete lines, parses them, and advances the
    /// offset by exactly the bytes accounted for — so a batch cut short by
    /// [`MAX_LINES_PER_BATCH`] resumes at a line boundary next tick.
    fn consume(&mut self, chunk: &[u8]) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut consumed = 0;

        for piece in chunk.split_inclusive(|byte| *byte == b'\n') {
            if lines.len() >= MAX_LINES_PER_BATCH {
                break;
            }

            consumed += piece.len();

            let Some(complete) = piece.strip_suffix(b"\n") else {
                if !self.discard_partial {
                    self.pending.extend_from_slice(piece);

                    if self.pending.len() > MAX_PENDING_BYTES {
                        lines.push(self.truncate_pending());
                    }
                }

                break;
            };

            if self.discard_partial {
                self.discard_partial = false;
                continue;
            }

            self.pending.extend_from_slice(
                complete.strip_suffix(b"\r").unwrap_or(complete),
            );
            lines.push(self.emit());
        }

        self.offset += consumed as u64;
        self.remember(&chunk[..consumed]);
        lines
    }

    /// Parses what is buffered as one line and files it under its module.
    ///
    /// Invalid UTF-8 is decoded lossily here and only here: it is a torn
    /// multi-byte write, not a reason to stop reading the daemon's log for
    /// the rest of the session, and by this point both halves of a character
    /// split across two reads have been reassembled.
    fn emit(&mut self) -> Line {
        let text = String::from_utf8_lossy(&std::mem::take(&mut self.pending))
            .into_owned();
        let record = log::parse_continuing(&text, self.previous.as_ref());

        self.previous = Some(record.clone());

        Line {
            module: record.module.clone().unwrap_or_else(ModuleId::daemon),
            record,
        }
    }

    /// Gives up on a line that has grown past [`MAX_PENDING_BYTES`] with no
    /// newline in sight: emits what arrived, says it was cut, and resyncs at
    /// the next newline. Discarding it instead would leave a module that
    /// emits newline-free output looking silent while its bytes piled up.
    fn truncate_pending(&mut self) -> Line {
        self.pending.extend_from_slice(TRUNCATION_MARKER.as_bytes());
        self.discard_partial = true;

        self.emit()
    }

    /// Keeps the last [`TAIL_BYTES`] of what the offset has passed over, as
    /// the fingerprint [`Tailer::tail_matches`] checks.
    fn remember(&mut self, consumed: &[u8]) {
        self.consumed_tail.extend_from_slice(consumed);

        if self.consumed_tail.len() > TAIL_BYTES {
            self.consumed_tail
                .drain(..self.consumed_tail.len() - TAIL_BYTES);
        }
    }

    /// A line the tailer invented. Marked `Unenveloped`/`Freeform` because
    /// that is what it is — nothing in the file said it — and attributed to
    /// the daemon, whose lifecycle it describes.
    fn synthetic(&self, level: Level, message: String) -> Line {
        Line {
            module: ModuleId::daemon(),
            record: Record {
                form: Form::Unenveloped,
                timestamp: Some(Local::now().naive_local()),
                stream: None,
                module: Some(ModuleId::daemon()),
                envelope_level: level,
                level,
                level_source: LevelSource::Envelope,
                sublanguage: Sublanguage::Freeform,
                target: None,
                message,
            },
        }
    }
}

/// Follows `path` forever, yielding one batch per [`POLL_INTERVAL`] in which
/// anything was written.
///
/// The stream never ends and never errors: a daemon that has not started, has
/// stopped, or has been restarted are all states the tailer sits through, and
/// ending the stream on any of them would mean the app stops seeing logs the
/// moment it most wants them.
pub fn follow(path: impl Into<PathBuf>) -> impl Stream<Item = Vec<Line>> {
    futures::stream::unfold(Tailer::new(path), |mut tailer| async move {
        loop {
            // Debounce first: the cost is a quarter second of latency on the
            // cold-start backfill, and the benefit is that a burst is always
            // one message rather than one per line.
            tokio::time::sleep(POLL_INTERVAL).await;

            let batch = tailer.poll().await;

            if !batch.is_empty() {
                return Some((batch, tailer));
            }
        }
    })
}

async fn read_at(
    path: &Path,
    offset: u64,
    budget: u64,
) -> std::io::Result<Vec<u8>> {
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(SeekFrom::Start(offset)).await?;

    let mut chunk = Vec::new();
    file.take(budget).read_to_end(&mut chunk).await?;

    Ok(chunk)
}

#[cfg(unix)]
fn identity(metadata: &std::fs::Metadata) -> Identity {
    use std::os::unix::fs::MetadataExt;

    Identity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(not(unix))]
fn identity(metadata: &std::fs::Metadata) -> Identity {
    Identity {
        created: metadata.created().ok(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;

    use futures::StreamExt;
    use tempfile::TempDir;

    use super::*;

    const FIXTURE: &str =
        include_str!("../../../src/testkit/fixtures/module_logs.txt");

    struct Log {
        _dir: TempDir,
        path: PathBuf,
    }

    impl Log {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();

            Self {
                path: dir.path().join("logoscore.log"),
                _dir: dir,
            }
        }

        fn append(&self, text: &str) {
            self.append_bytes(text.as_bytes());
        }

        /// For the torn writes a `&str` cannot express: half a character.
        fn append_bytes(&self, bytes: &[u8]) {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .unwrap()
                .write_all(bytes)
                .unwrap();
        }

        fn appendln(&self, line: &str) {
            self.append(&format!("{line}\n"));
        }

        /// What the supervisor does on every spawn: same path, same inode,
        /// length back to zero.
        fn truncate(&self) {
            std::fs::File::create(&self.path).unwrap();
        }

        fn tailer(&self) -> Tailer {
            Tailer::new(&self.path)
        }
    }

    fn fixture_lines() -> Vec<&'static str> {
        FIXTURE.lines().collect()
    }

    fn messages(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(Line::message).collect()
    }

    #[tokio::test]
    async fn missing_file_is_not_an_error() {
        let log = Log::new();
        let mut tailer = log.tailer();

        assert!(tailer.poll().await.is_empty());
        assert!(tailer.poll().await.is_empty());

        log.appendln(fixture_lines()[3]);

        let batch = tailer.poll().await;

        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].module, ModuleId::from("capability_module"));
    }

    #[tokio::test]
    async fn appends_arrive_in_order_and_only_once() {
        let log = Log::new();
        log.appendln(fixture_lines()[3]);

        let mut tailer = log.tailer();
        let first = tailer.poll().await;

        assert_eq!(first.len(), 1);
        assert!(tailer.poll().await.is_empty());

        log.appendln(fixture_lines()[4]);
        log.appendln(fixture_lines()[5]);

        let second = tailer.poll().await;

        assert_eq!(
            messages(&second),
            vec![
                "Module loaded: blockchain_module",
                "[LogosProviderObject] LogosAPIProvider: detected \
                 LogosProviderPlugin for \"blockchain_module\"",
            ]
        );
        assert!(tailer.poll().await.is_empty());
    }

    /// The daemon's two writers share one file description with no
    /// `O_APPEND`, so a read landing mid-line is expected rather than
    /// exotic. Half an envelope parsed as a line would be attributed to the
    /// wrong module and rendered at the wrong level.
    #[tokio::test]
    async fn a_partial_line_is_held_until_its_newline() {
        let log = Log::new();
        let line = fixture_lines()[47];
        let (head, tail) = line.split_at(40);

        log.append(head);

        let mut tailer = log.tailer();

        assert!(tailer.poll().await.is_empty());

        log.append(tail);

        assert!(tailer.poll().await.is_empty());

        log.append("\n");

        let batch = tailer.poll().await;

        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].module, ModuleId::from("delivery_module"));
        assert_eq!(batch[0].level(), Level::Warn);
    }

    /// `File::create` on respawn keeps the inode and resets the length. A
    /// tailer that only ever seeks forward would read nothing for the rest of
    /// the session — the failure is silent, which is why it is worth a test.
    ///
    /// The respawn here also outruns the reader: by the time the tailer
    /// looks, the new run has written past the old offset, so neither the
    /// inode nor the length betrays the reset. Only the head fingerprint
    /// does, and this is the ordinary case for a daemon restarted while the
    /// app is up.
    #[tokio::test]
    async fn truncation_reopens_from_the_start() {
        let log = Log::new();

        for _ in 0..20 {
            log.appendln(fixture_lines()[3]);
        }

        let mut tailer = log.tailer();

        assert_eq!(tailer.poll().await.len(), 20);

        log.truncate();
        log.appendln(fixture_lines()[0]);
        log.appendln(fixture_lines()[4]);

        for _ in 0..40 {
            log.appendln(fixture_lines()[47]);
        }

        let batch = tailer.poll().await;

        assert_eq!(batch.len(), 43);

        assert_eq!(batch[0].message(), RESTART_MARKER);
        assert_eq!(batch[0].module, ModuleId::daemon());
        assert_eq!(
            messages(&batch[1..3]),
            vec![
                "Logoscore daemon started (pid 37910, instance a8c1fd47d643)",
                "Module loaded: blockchain_module",
            ]
        );
    }

    /// A replaced file (a new instance dir, or a log removed and recreated)
    /// is the same reset as a truncation even though the length never drops.
    #[tokio::test]
    async fn a_replaced_file_is_read_from_the_start() {
        let log = Log::new();
        log.appendln(fixture_lines()[3]);

        let mut tailer = log.tailer();

        assert_eq!(tailer.poll().await.len(), 1);

        std::fs::remove_file(&log.path).unwrap();
        log.appendln(fixture_lines()[3]);
        log.appendln(fixture_lines()[4]);

        let batch = tailer.poll().await;

        assert_eq!(batch.len(), 3);
        assert_eq!(batch[0].message(), RESTART_MARKER);
    }

    /// A burst must not become one enormous message, and the remainder must
    /// not be skipped: the cap bounds the batch, not the read.
    #[tokio::test]
    async fn a_burst_is_split_across_ticks_without_loss() {
        let log = Log::new();
        let line = fixture_lines()[47];
        let total = MAX_LINES_PER_BATCH + 20;

        for _ in 0..total {
            log.appendln(line);
        }

        let mut tailer = log.tailer();
        let first = tailer.poll().await;
        let second = tailer.poll().await;

        assert_eq!(first.len(), MAX_LINES_PER_BATCH);
        assert_eq!(second.len(), 20);
        assert!(tailer.poll().await.is_empty());
        assert!(
            first
                .iter()
                .chain(second.iter())
                .all(|line| line.module == ModuleId::from("delivery_module"))
        );
    }

    /// The continuation rule is what keeps a multi-line panic attributed to
    /// the module that panicked. It has to survive the tick boundary, since
    /// where the reads fall is not something the daemon coordinates with.
    #[tokio::test]
    async fn continuation_attribution_survives_a_tick_boundary() {
        let log = Log::new();
        let crash = fixture_lines()
            .into_iter()
            .position(|line| line.contains("panicked at"))
            .expect("fixture carries the crash sequence");

        log.appendln(fixture_lines()[crash]);

        let mut tailer = log.tailer();
        let first = tailer.poll().await;

        assert_eq!(first[0].module, ModuleId::from("blockchain_module"));

        log.appendln("   0: 0x102f4a1c0 - <unknown>");

        let second = tailer.poll().await;

        assert_eq!(second[0].module, ModuleId::from("blockchain_module"));
        assert_eq!(second[0].record.form, Form::Unenveloped);
    }

    /// Opening against a daemon that has been running for hours must not
    /// replay the whole file, and the line the seek lands inside must be
    /// dropped rather than parsed as a line beginning nowhere.
    #[tokio::test]
    async fn a_cold_start_reads_only_the_tail() {
        let log = Log::new();
        let filler = fixture_lines()[47];
        let repeats = (BACKFILL_BYTES as usize / filler.len()) + 200;

        for _ in 0..repeats {
            log.appendln(filler);
        }

        log.appendln(fixture_lines()[4]);

        let mut tailer = log.tailer();
        let mut seen = Vec::new();

        loop {
            let batch = tailer.poll().await;

            if batch.is_empty() {
                break;
            }

            seen.extend(batch);
        }

        assert!(seen.len() < repeats);
        assert_eq!(
            seen.last().unwrap().message(),
            "Module loaded: blockchain_module"
        );
        assert!(
            seen.iter()
                .all(|line| line.record.form != Form::Unenveloped),
            "the partial line the backfill seek landed in was parsed"
        );
    }

    #[tokio::test]
    async fn follow_yields_batches_as_the_file_grows() {
        let log = Log::new();
        let mut stream = Box::pin(follow(log.path.clone()));

        log.appendln(fixture_lines()[3]);
        log.appendln(fixture_lines()[4]);

        let batch = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("follow produced a batch within the timeout")
            .expect("the stream never ends");

        assert_eq!(batch.len(), 2);
    }

    /// Every line the real corpus contains must come out of the tailer with
    /// an owner *and* be there when the pane asks for it. Attribution and
    /// retention are one property from the pane's point of view: a line filed
    /// under the wrong module and a line the history swallowed are the same
    /// missing row.
    ///
    /// The counts are the parser's verified ones. The corpus is what makes
    /// this a real test of the second half: it contains the tip-request error
    /// twice per peer, half a second apart, which is exactly the shape chat's
    /// duplicate heuristic used to discard.
    #[tokio::test]
    async fn every_fixture_line_reaches_the_history_its_pane_reads() {
        let log = Log::new();
        log.append(FIXTURE);

        let mut tailer = log.tailer();
        let mut history = crate::history::Manager::default();

        loop {
            let batch = tailer.poll().await;

            if batch.is_empty() {
                break;
            }

            for line in batch {
                history.record_module_log(line);
            }
        }

        let held = |id: ModuleId| {
            history
                .get_messages(&crate::history::Kind::Module(id), None)
                .map_or(0, |view| view.total)
        };

        assert_eq!(
            held(ModuleId::from("blockchain_module")),
            37,
            "attribution or retention drifted from the parser's counts"
        );
        assert_eq!(held(ModuleId::from("delivery_module")), 8);
        assert_eq!(held(ModuleId::from("chat_module")), 3);
        assert_eq!(held(ModuleId::from("capability_module")), 2);
        assert_eq!(held(ModuleId::daemon()), 5);

        // Summed over every history there is, so a line filed under an id
        // the assertions above do not name is still missing from the total.
        let total: usize = history
            .kinds()
            .iter()
            .map(|kind| {
                history
                    .get_messages(kind, None)
                    .map_or(0, |view| view.total)
            })
            .sum();

        assert_eq!(
            total,
            FIXTURE.lines().count(),
            "the corpus lost lines between the file and the history"
        );

        let view = history
            .get_messages(
                &crate::history::Kind::Module(ModuleId::from(
                    "blockchain_module",
                )),
                None,
            )
            .unwrap();

        assert_eq!(
            view.old_messages
                .iter()
                .chain(&view.new_messages)
                .filter(|message| {
                    message
                        .content
                        .text()
                        .starts_with("Error while processing tip request")
                })
                .count(),
            8,
            "the repeated per-peer failure is the signal, not noise: four \
             peers, twice each",
        );
    }

    /// Newline-free output used to be absorbed forever: the offset advanced
    /// past every byte, so the backlog never grew and nothing was ever
    /// emitted while the buffer climbed at the daemon's write rate. The cap
    /// has to both bound the buffer and put the bytes on screen.
    #[tokio::test]
    async fn newline_free_output_is_capped_and_emitted() {
        let log = Log::new();
        let mut tailer = log.tailer();
        let piece = "x".repeat(32 * 1024);
        let mut seen = Vec::new();

        for _ in 0..8 {
            log.append(&piece);
            seen.extend(tailer.poll().await);
        }

        assert!(
            tailer.pending.len() <= MAX_PENDING_BYTES,
            "the buffer grew past its cap"
        );
        assert!(
            !seen.is_empty(),
            "256 KiB of newline-free output emitted nothing"
        );
        assert!(
            seen.iter()
                .all(|line| line.message().ends_with(TRUNCATION_MARKER.trim())),
            "the fragment was emitted without saying it was cut"
        );

        log.appendln("");
        log.appendln(fixture_lines()[4]);

        let batch = tailer.poll().await;

        assert_eq!(messages(&batch), vec!["Module loaded: blockchain_module"]);
        assert!(tailer.pending.is_empty());
    }

    /// A respawn whose first [`HEAD_BYTES`] bytes repeat the previous run's
    /// is invisible to the head fingerprint, and outrunning the reader hides
    /// it from the inode and length checks too. What gives it away is that
    /// the bytes under the read offset are not the bytes we read there.
    #[tokio::test]
    async fn a_respawn_that_repeats_the_head_is_still_detected() {
        let log = Log::new();
        let banner = fixture_lines()[3];

        for _ in 0..40 {
            log.appendln(banner);
        }

        let head_before =
            std::fs::read(&log.path).unwrap()[..HEAD_BYTES as usize].to_vec();

        let mut tailer = log.tailer();

        assert_eq!(tailer.poll().await.len(), 40);

        log.truncate();

        for _ in 0..5 {
            log.appendln(banner);
        }

        for _ in 0..60 {
            log.appendln(fixture_lines()[47]);
        }

        assert_eq!(
            std::fs::read(&log.path).unwrap()[..HEAD_BYTES as usize],
            head_before[..],
            "the test no longer reproduces the head, so it proves nothing"
        );

        let mut seen = Vec::new();

        loop {
            let batch = tailer.poll().await;

            if batch.is_empty() {
                break;
            }

            seen.extend(batch);
        }

        assert_eq!(seen[0].message(), RESTART_MARKER);
        assert_eq!(seen.len(), 66);
    }

    /// Both halves of a character split across two reads arrive; decoding
    /// each read on its own destroyed them anyway. Decoding once, on the
    /// assembled line, is the whole fix.
    #[tokio::test]
    async fn a_character_split_across_polls_survives() {
        let log = Log::new();
        let line = "backtrace café ✅";
        let (head, tail) = line.as_bytes().split_at(line.len() - 1);

        log.append_bytes(head);

        let mut tailer = log.tailer();

        assert!(tailer.poll().await.is_empty());

        log.append_bytes(&[tail, b"\n"].concat());

        let batch = tailer.poll().await;

        assert_eq!(messages(&batch), vec![line]);
    }

    /// A backfill offset that lands exactly on a newline starts a whole line,
    /// not a fragment — discarding to the next newline there loses one
    /// complete line on every cold start that happens to be aligned.
    #[tokio::test]
    async fn a_backfill_landing_on_a_newline_keeps_the_next_line() {
        let log = Log::new();
        let width = 128;
        let lines = (BACKFILL_BYTES as usize / width) + 10;

        for index in 0..lines {
            let head = format!("line {index:06}");

            log.appendln(&format!(
                "{head}{}",
                "-".repeat(width - head.len() - 1)
            ));
        }

        let mut tailer = log.tailer();
        let mut seen = Vec::new();

        loop {
            let batch = tailer.poll().await;

            if batch.is_empty() {
                break;
            }

            seen.extend(batch);
        }

        assert_eq!(tailer.offset % width as u64, 0, "the seek was not aligned");
        assert_eq!(seen.len(), BACKFILL_BYTES as usize / width);
        assert!(seen[0].message().starts_with("line 000010"));
    }
}
