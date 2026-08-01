//! Settings for the module monitor's log panes.

use serde::Deserialize;

use crate::config::logs::LevelFilter;
use crate::module::log::Level;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Modules {
    /// Lowest severity a module log line may carry and still reach a pane.
    ///
    /// Not optional and not cosmetic. A blockchain node ships with
    /// `tracing.logger.level: DEBUG`, and delivery's observed output was 38%
    /// `DBG` — 753 of 1976 lines in a few minutes (`logos-modules.md` §4). Without a
    /// floor the pane is a firehose that evicts its own interesting lines.
    pub log_level: LevelFilter,
}

impl Default for Modules {
    fn default() -> Self {
        Self {
            log_level: LevelFilter::Info,
        }
    }
}

impl Modules {
    /// Whether a line at `level` is worth rendering.
    ///
    /// Applied before a `data::Message` is allocated, so filtered lines cost
    /// only their parse. `Off` admits nothing, matching what the same word
    /// means for the app's own log.
    pub fn admits(&self, level: Level) -> bool {
        let floor = match self.log_level {
            LevelFilter::Off => return false,
            LevelFilter::Error => Level::Error,
            LevelFilter::Warn => Level::Warn,
            LevelFilter::Info => Level::Info,
            LevelFilter::Debug => Level::Debug,
            LevelFilter::Trace => Level::Trace,
        };

        level >= floor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Level` is `Ord` precisely so a floor is a comparison; this pins that
    /// the ordering runs the way the filter reads.
    #[test]
    fn the_default_floor_admits_info_and_above_and_nothing_below() {
        let modules = Modules::default();

        assert!(modules.admits(Level::Critical));
        assert!(modules.admits(Level::Error));
        assert!(modules.admits(Level::Warn));
        assert!(modules.admits(Level::Info));
        assert!(!modules.admits(Level::Debug));
        assert!(!modules.admits(Level::Trace));
    }

    /// A crash arrives as `Critical`, so the strictest usable floor must
    /// still let it through — `logos-modules.md` §5 is the case the pane exists for.
    #[test]
    fn a_crash_survives_every_floor_but_off() {
        for filter in [
            LevelFilter::Error,
            LevelFilter::Warn,
            LevelFilter::Info,
            LevelFilter::Debug,
            LevelFilter::Trace,
        ] {
            let label = format!("{filter:?}");

            assert!(
                Modules { log_level: filter }.admits(Level::Critical),
                "a crash line was filtered out at {label}",
            );
        }

        assert!(
            !Modules {
                log_level: LevelFilter::Off
            }
            .admits(Level::Critical)
        );
    }
}
