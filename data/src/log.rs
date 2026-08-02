use std::cmp::Ordering;
use std::path::PathBuf;
use std::{fs, io};

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

use crate::config::logs::{LevelFilter, Timestamp};
use crate::{environment, module};

pub fn file(timestamp: Timestamp) -> Result<fs::File, Error> {
    let file_format = "frigicom.%Y-%m-%d-%H-%M-%S.log";
    let path = dir()?.join(
        match timestamp {
            Timestamp::Local => Local::now().format(file_format),
            Timestamp::Utc => Utc::now().format(file_format),
        }
        .to_string(),
    );

    Ok(fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(false)
        .truncate(true)
        .open(path)?)
}

fn dir() -> Result<PathBuf, Error> {
    let dir = environment::data_dir().join("logs");

    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }

    Ok(dir)
}

pub fn clear(number_of_logs_to_keep: usize) {
    if let Ok(dir) = dir() {
        for (index, dir_entry) in walkdir::WalkDir::new(dir)
            .max_depth(1)
            .sort_by(|a, b| b.file_name().cmp(a.file_name()))
            .into_iter()
            .filter_map(Result::ok)
            .filter(|dir_entry| {
                dir_entry.file_type().is_file()
                    && dir_entry.file_name().to_str().is_some_and(|file_name| {
                        file_name.starts_with("frigicom.")
                            && file_name.ends_with(".log")
                    })
            })
            .enumerate()
        {
            if index >= number_of_logs_to_keep {
                let _ = fs::remove_file(dir_entry.path());
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Record {
    pub timestamp: DateTime<Utc>,
    pub level: Level,
    pub message: String,
}

#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Debug,
    Hash,
    Serialize,
    Deserialize,
    strum::Display,
)]
#[strum(serialize_all = "UPPERCASE")]
pub enum Level {
    /// A module dying.
    ///
    /// The app's own logger can never produce one — the `log` crate has five
    /// levels and [`From<log::Level>`] is total over them. It is here because
    /// this is the level the log panes render from, and a module abort
    /// (`logos-modules.md` §5) arriving as
    /// [`module::log::Level::Critical`](crate::module::log::Level) must not
    /// be flattened into the same word and colour as the routine errors it
    /// sits among.
    ///
    /// Rendered as `FATAL`: it is the daemon's own word for the event, and it
    /// is five characters, so the fixed-width severity column that every log
    /// row aligns on does not have to widen for the rarest level in it.
    #[strum(serialize = "FATAL")]
    Critical,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<log::Level> for Level {
    fn from(level: log::Level) -> Self {
        match level {
            log::Level::Error => Level::Error,
            log::Level::Warn => Level::Warn,
            log::Level::Info => Level::Info,
            log::Level::Debug => Level::Debug,
            log::Level::Trace => Level::Trace,
        }
    }
}

/// Bridges a module log's severity onto the one the panes render from
/// (`theme::selectable_text::log_level`).
///
/// It lives on this side of the licence boundary because `Level` is the
/// local type here, which is what the orphan rule wants — `logos-domain`
/// cannot implement it and says so where the impl used to be.
///
/// One-to-one, deliberately. An earlier version folded `Critical` into
/// `Error`, which cost nothing in the common case and everything in the one
/// that matters: a module abort (`logos-modules.md` §5) came out of the pane
/// wearing the same word and the same colour as the routine errors above it,
/// so the line the monitor exists to surface was the hardest one to find.
impl From<module::log::Level> for Level {
    fn from(level: module::log::Level) -> Self {
        match level {
            module::log::Level::Trace => Level::Trace,
            module::log::Level::Debug => Level::Debug,
            module::log::Level::Info => Level::Info,
            module::log::Level::Warn => Level::Warn,
            module::log::Level::Error => Level::Error,
            module::log::Level::Critical => Level::Critical,
        }
    }
}

impl std::cmp::PartialOrd<LevelFilter> for Level {
    fn partial_cmp(&self, other: &LevelFilter) -> Option<Ordering> {
        Some(match self {
            // `LevelFilter` names no ceiling above `Error`, so a crash sorts
            // below every floor that admits anything at all and above `Off`
            // alone — the same rule `config::Modules::admits` applies to the
            // module-side level, stated as an ordering.
            Level::Critical => match other {
                LevelFilter::Off => Ordering::Greater,
                LevelFilter::Error
                | LevelFilter::Warn
                | LevelFilter::Info
                | LevelFilter::Debug
                | LevelFilter::Trace => Ordering::Less,
            },
            Level::Error => match other {
                LevelFilter::Off => Ordering::Greater,
                LevelFilter::Error => Ordering::Equal,
                LevelFilter::Warn
                | LevelFilter::Info
                | LevelFilter::Debug
                | LevelFilter::Trace => Ordering::Less,
            },
            Level::Warn => match other {
                LevelFilter::Off | LevelFilter::Error => Ordering::Greater,
                LevelFilter::Warn => Ordering::Equal,
                LevelFilter::Info | LevelFilter::Debug | LevelFilter::Trace => {
                    Ordering::Less
                }
            },
            Level::Info => match other {
                LevelFilter::Off | LevelFilter::Error | LevelFilter::Warn => {
                    Ordering::Greater
                }
                LevelFilter::Info => Ordering::Equal,
                LevelFilter::Debug | LevelFilter::Trace => Ordering::Less,
            },
            Level::Debug => match other {
                LevelFilter::Off
                | LevelFilter::Error
                | LevelFilter::Warn
                | LevelFilter::Info => Ordering::Greater,
                LevelFilter::Debug => Ordering::Equal,
                LevelFilter::Trace => Ordering::Less,
            },
            Level::Trace => match other {
                LevelFilter::Off
                | LevelFilter::Error
                | LevelFilter::Warn
                | LevelFilter::Info
                | LevelFilter::Debug => Ordering::Greater,
                LevelFilter::Trace => Ordering::Equal,
            },
        })
    }
}

impl std::cmp::PartialEq<LevelFilter> for Level {
    fn eq(&self, other: &LevelFilter) -> bool {
        match self {
            // No filter selects crashes and nothing else, so `Critical`
            // equals none of them; it is strictly above `Error`, never at it.
            Level::Critical => false,
            Level::Error => matches!(other, LevelFilter::Error),
            Level::Warn => matches!(other, LevelFilter::Warn),
            Level::Info => matches!(other, LevelFilter::Info),
            Level::Debug => matches!(other, LevelFilter::Debug),
            Level::Trace => matches!(other, LevelFilter::Trace),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    SetLog(#[from] log::SetLoggerError),
    #[error(transparent)]
    ParseLevel(#[from] log::ParseLevelError),
}
