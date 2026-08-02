//! What the domain crate cannot assert about itself.
//!
//! `logos-domain` is a separate, permissively licensed crate that knows
//! nothing about this one — so the two places its output meets halloy-derived
//! code are only testable from here. Both of these lived inside the domain
//! modules before the split and assert exactly what they did then.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use data::module::ModuleId;
use data::module::log::{Level, parse};
use data::module::tail::Tailer;
use logos_domain::fixtures::MODULE_LOGS as FIXTURE;
use tempfile::TempDir;

/// A throwaway daemon log to tail. The domain crate has the same helper for
/// its own tests; it is private there, and duplicating six lines is cheaper
/// than widening a public API for a test.
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
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    fn tailer(&self) -> Tailer {
        Tailer::new(&self.path)
    }
}

/// The bridge to the render level is one-to-one. It used to fold
/// `Critical` into `Error`, which made the abort in `logos-modules.md` §5
/// indistinguishable from the routine errors it sits among; every other
/// level maps to its own name and is pinned here so the fold cannot come
/// back as a catch-all arm.
#[test]
fn the_render_level_keeps_a_crash_apart_from_an_error() {
    for (level, rendered) in [
        (Level::Trace, data::log::Level::Trace),
        (Level::Debug, data::log::Level::Debug),
        (Level::Info, data::log::Level::Info),
        (Level::Warn, data::log::Level::Warn),
        (Level::Error, data::log::Level::Error),
        (Level::Critical, data::log::Level::Critical),
    ] {
        assert_eq!(data::log::Level::from(level), rendered);
    }

    let notice = parse(
        "[2026-07-31 21:27:53.229] [critical] [logos] \
         Module process crashed: blockchain_module",
    );

    assert!(notice.reports_crash());
    assert_eq!(
        data::log::Level::from(notice.level),
        data::log::Level::Critical,
    );
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
    let mut history = data::history::Manager::default();

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
            .get_messages(&data::history::Kind::Module(id), None)
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
            &data::history::Kind::Module(ModuleId::from("blockchain_module")),
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
