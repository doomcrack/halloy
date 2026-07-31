use serde::Deserialize;

use crate::buffer::{Alignment, Brackets, Color};
use crate::config::buffer::HideConsecutive;

/// Styling for the message sender label (the IRC-era `nickname` block).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Sender {
    pub color: Color,
    pub brackets: Brackets,
    pub alignment: Alignment,
    pub truncate: Option<u16>,
    pub hide_consecutive: HideConsecutive,
}
