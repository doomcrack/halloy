use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

use crate::buffer::{self, Buffer};
use crate::pane::Pane;
use crate::{compression, environment};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Dashboard {
    pub pane: Pane,
    pub popout_panes: Vec<Pane>,
    pub buffer_settings: BufferSettings,
    pub focus_buffer: Option<Buffer>,
    pub sidebar: Sidebar,
    /// Whether the "adding a member takes up to a minute" explainer has
    /// been acknowledged with don't-show-again (QML `memberAddExplained`).
    pub member_add_explained: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BufferSettings {
    settings: HashMap<String, buffer::Settings>,
    pub show_muted: bool,
}

impl<'de> Deserialize<'de> for BufferSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Debug, Clone, Deserialize)]
        #[serde(untagged)]
        pub enum Format {
            BufferSettings {
                #[serde(default)]
                settings: HashMap<String, buffer::Settings>,
                #[serde(default)]
                show_muted: bool,
            },
            Legacy(HashMap<String, buffer::Settings>),
        }

        match Format::deserialize(deserializer)? {
            Format::BufferSettings {
                settings,
                show_muted,
            } => Ok(BufferSettings {
                settings,
                show_muted,
            }),
            Format::Legacy(settings) => Ok(BufferSettings {
                settings,
                ..BufferSettings::default()
            }),
        }
    }
}

impl BufferSettings {
    pub fn get(&self, buffer: &buffer::Buffer) -> Option<&buffer::Settings> {
        self.settings.get(&buffer.key())
    }

    /// Drops persisted per-conversation settings. Identity is ephemeral
    /// upstream, so `convo:` keys from a previous run can never match a
    /// live conversation again and would otherwise grow unboundedly.
    ///
    /// Only `convo:` keys. Module ids are stable across restarts by
    /// construction — the catalog is hardcoded — so a `module:` key still
    /// names the same module next launch and must survive, as must the
    /// internal buffers'.
    ///
    /// Runs at load, where the live set is not yet known — nothing
    /// conversation-keyed here survives to be spared. Layout and focus
    /// cannot be swept the same way (a pane is the user's, not a cache
    /// entry), so they are reconciled against the first snapshot instead;
    /// see `screen::Dashboard::reconcile_conversations`.
    fn prune_orphaned_conversations(&mut self) {
        self.settings.retain(|key, _| !key.starts_with("convo:"));
    }

    pub fn entry(
        &mut self,
        buffer: &buffer::Buffer,
        maybe_default: Option<buffer::Settings>,
    ) -> &mut buffer::Settings {
        self.settings
            .entry(buffer.key())
            .or_insert_with(|| maybe_default.unwrap_or_default())
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sidebar {
    Hidden,
    #[default]
    Visible,
}

impl Sidebar {
    pub fn is_hidden(self) -> bool {
        matches!(self, Self::Hidden)
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BufferAction {
    #[default]
    NewPane,
    ReplacePane,
    NewWindow,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BufferFocusedAction {
    #[default]
    ClosePane,
}

impl Dashboard {
    pub fn exists() -> Result<bool, Error> {
        let path = path()?;

        Ok(std::fs::exists(path)?)
    }

    pub fn load() -> Result<Self, Error> {
        let path = path()?;

        let bytes = std::fs::read(path)?;

        let mut dashboard: Self = compression::decompress(&bytes)?;
        dashboard.buffer_settings.prune_orphaned_conversations();

        Ok(dashboard)
    }

    pub async fn save(self) -> Result<(), Error> {
        let path = path()?;

        let bytes = compression::compress(&self)?;

        tokio::fs::write(path, &bytes).await?;

        Ok(())
    }
}

fn path() -> Result<PathBuf, Error> {
    let parent = environment::data_dir();

    if !parent.exists() {
        std::fs::create_dir_all(&parent)?;
    }

    Ok(parent.join("dashboard.json.gz"))
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Compression(#[from] compression::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::ConvoId;
    use crate::module::ModuleId;
    use crate::pane::Pane;

    /// The load-time sweep exists because conversation ids are ephemeral.
    /// Module ids are not — the catalog is hardcoded precisely so a module
    /// pane means the same thing next launch — so they must survive it, as
    /// must the internal buffers'.
    #[test]
    fn pruning_orphaned_conversations_spares_module_settings() {
        let mut settings = BufferSettings::default();

        for buffer in [
            Buffer::Conversation(ConvoId::from("abc123")),
            Buffer::Module(ModuleId::from("blockchain_module")),
            Buffer::Internal(buffer::Internal::Logs),
        ] {
            settings.entry(&buffer, None);
        }

        settings.prune_orphaned_conversations();

        assert!(
            settings
                .get(&Buffer::Conversation(ConvoId::from("abc123")))
                .is_none()
        );
        assert!(
            settings
                .get(&Buffer::Module(ModuleId::from("blockchain_module")))
                .is_some()
        );
        assert!(
            settings
                .get(&Buffer::Internal(buffer::Internal::Logs))
                .is_some()
        );
    }

    /// A module pane and a module focus must come back exactly as they went
    /// in — nothing on the persistence path is allowed to prune them.
    #[test]
    fn module_panes_survive_the_persisted_round_trip() {
        let module = Buffer::Module(ModuleId::from("blockchain_module"));
        let dashboard = Dashboard {
            pane: Pane::Buffer {
                buffer: module.clone(),
            },
            focus_buffer: Some(module.clone()),
            ..Dashboard::default()
        };

        let bytes = compression::compress(&dashboard).unwrap();
        let mut restored: Dashboard = compression::decompress(&bytes).unwrap();
        restored.buffer_settings.prune_orphaned_conversations();

        assert!(matches!(
            restored.pane,
            Pane::Buffer { buffer } if buffer == module
        ));
        assert_eq!(restored.focus_buffer, Some(module));
    }
}
