//! The blockchain node's status, above its log.
//!
//! An operator view, so density over decoration: what mode the node is in,
//! how far through the chain it is, and the newest blocks it has. Three
//! rules, each of them a measured failure rather than a preference:
//!
//! - **`Bootstrapping` is not a stall.** It lasts tens of minutes on a cold
//!   start and the node is working the whole time. The word alone reads as
//!   a hang, so height and slot sit beside it and visibly move.
//! - **Absence is not zero.** There is no peer count in this build at all,
//!   and no finalised blocks until the node is caught up. Both say so.
//! - **The log stays.** It is the only place a crash is visible, so the
//!   panel sits above it rather than replacing it.

use data::Module;
use logos_blockchain_client::{Block, CryptarchiaInfo, Probe, State};

use crate::Theme;
use crate::widget::Element;
use crate::widget::panel::{self, Cell, Tone};

/// How much of a hash to show. Enough to recognise and compare at a glance;
/// the full value is not useful in a column.
const HASH: usize = 8;

/// Whether this module gets a panel. Only the blockchain node has one so
/// far — the others would show an empty frame, which is worse than the log
/// they already have.
pub fn applies(module: &Module) -> bool {
    module.id.as_str() == logos_blockchain_client::MODULE
}

pub fn view<'a, M: 'a>(state: &'a State, theme: &'a Theme) -> Element<'a, M> {
    let Some(sample) = state.latest() else {
        // Before the first poll. Not an error and not a node problem —
        // the monitor pass simply has not run yet.
        return panel::absent("Waiting for the first reading…", theme);
    };

    let mut sections: Vec<Element<'a, M>> =
        vec![stats(state, &sample.info, &sample.peer_id, theme)];

    if let Some(bar) = progress(state, theme) {
        sections.push(bar);
    }

    sections.push(blocks(&sample.blocks, theme));

    iced::widget::column(sections).spacing(10).into()
}

fn stats<'a, M: 'a>(
    state: &'a State,
    info: &'a Probe<CryptarchiaInfo>,
    peers: &'a Probe<String>,
    theme: &'a Theme,
) -> Element<'a, M> {
    let (mode, height, slot, tip, lib, mode_tone) = match info.ready() {
        Some(info) => (
            Some(info.mode.clone()),
            Some(thousands(info.height)),
            Some(thousands(info.slot)),
            Some(short(&info.tip)),
            Some(short(&info.lib)),
            if info.is_online() {
                Tone::Good
            } else {
                Tone::Working
            },
        ),
        None => (None, None, None, None, None, Tone::Absent),
    };

    let note = info.note().unwrap_or("unavailable");
    let pulses = thousands(state.total_pulses());

    panel::stat_grid(
        vec![
            panel::stat("mode", mode, Some(note), mode_tone, theme),
            panel::stat("height", height, Some(note), Tone::Neutral, theme),
            panel::stat("slot", slot, Some(note), Tone::Neutral, theme),
            // Counted, never rendered per arrival: 1158 of these landed in
            // one catch-up, and a repaint each would thrash the pane.
            panel::stat(
                "blocks seen",
                Some(pulses),
                None,
                Tone::Neutral,
                theme,
            ),
            panel::stat("tip", tip, Some(note), Tone::Neutral, theme),
            panel::stat("finalised", lib, Some(note), Tone::Neutral, theme),
            // Rendered even though it never has a value. Omitting the row
            // would leave a reader wondering where the peer count went;
            // showing zero would be a false claim. It says which it is.
            panel::stat(
                "peers",
                peers.ready().cloned(),
                peers.note(),
                Tone::Neutral,
                theme,
            ),
        ],
        3,
    )
}

/// Sync progress, computed from the genesis constants because the node has
/// no `get_time_info` to ask.
fn progress<'a, M: 'a>(
    state: &'a State,
    theme: &'a Theme,
) -> Option<Element<'a, M>> {
    let fraction = state.progress()?;
    let remaining = state.remaining()?;

    // Once there is nothing left to cover the bar is noise.
    if remaining == 0 {
        return None;
    }

    let percent = format!(
        "{:.1}% · {} slots behind",
        fraction * 100.0,
        thousands(remaining)
    );

    Some(panel::stat(
        "catching up",
        Some(percent),
        None,
        Tone::Working,
        theme,
    ))
}

fn blocks<'a, M: 'a>(
    blocks: &'a Probe<Vec<Block>>,
    theme: &'a Theme,
) -> Element<'a, M> {
    let Some(ready) = blocks.ready() else {
        return panel::absent(blocks.note().unwrap_or("no blocks"), theme);
    };

    if ready.is_empty() {
        return panel::absent("no blocks yet", theme);
    }

    let rows: Vec<Vec<Cell>> = ready
        .iter()
        .map(|block| {
            vec![
                Cell::new(thousands(block.slot), 3),
                Cell::new(short(&block.id), 4),
                Cell::new(block.transactions.to_string(), 2),
                Cell::new(
                    block
                        .leader_key
                        .as_deref()
                        .map_or_else(|| "—".to_owned(), short),
                    4,
                )
                .tone(Tone::Absent),
            ]
        })
        .collect();

    panel::table(
        &[
            Cell::new("slot", 3),
            Cell::new("block", 4),
            Cell::new("tx", 2),
            Cell::new("leader", 4),
        ],
        &rows,
        theme,
    )
}

fn short(hash: &str) -> String {
    hash.chars().take(HASH).collect()
}

/// Group digits so a six-figure height is readable at a glance.
fn thousands(value: u64) -> String {
    let digits = value.to_string();

    digits
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_grouped() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(93_148), "93,148");
        assert_eq!(thousands(2_748_893), "2,748,893");
    }

    #[test]
    fn a_short_hash_is_not_padded_or_panicked_on() {
        assert_eq!(short("efa86ac70717d040"), "efa86ac7");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short(""), "");
    }
}
