use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Display {
    pub decode_urls: bool,
}

impl Default for Display {
    fn default() -> Self {
        Self { decode_urls: true }
    }
}
