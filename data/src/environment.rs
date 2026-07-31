use std::env;
use std::path::PathBuf;

pub const VERSION: &str = env!("VERSION");
pub const GIT_HASH: Option<&str> = option_env!("GIT_HASH");
pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const APPLICATION_ID: &str = "org.logos.frigicom";
pub const SOURCE_WEBSITE: &str = "https://github.com/doomcrack/halloy";
pub const WIKI_WEBSITE: &str =
    "https://github.com/doomcrack/halloy/tree/main/docs";
pub const RELEASE_WEBSITE: &str =
    "https://github.com/doomcrack/halloy/releases/latest";
/// Upstream halloy's theme gallery. Frigicom inherited halloy's theme
/// format unchanged, so the gallery and the `halloy:///theme` deep link
/// still work; there is no frigicom-specific gallery.
pub const THEME_WEBSITE: &str = "https://themes.halloy.chat";

const APPLICATION_DIR_NAME: &str = "frigicom";

pub fn formatted_version() -> String {
    let hash = GIT_HASH
        .map(|hash| format!(" ({hash})"))
        .unwrap_or_default();

    format!("{VERSION}{hash}")
}

pub fn config_dir() -> PathBuf {
    portable_dir().unwrap_or_else(platform_specific_config_dir)
}

pub fn data_dir() -> PathBuf {
    portable_dir().unwrap_or_else(|| {
        dirs_next::data_dir()
            .expect("expected valid data dir")
            .join(APPLICATION_DIR_NAME)
    })
}

pub fn cache_dir() -> PathBuf {
    dirs_next::cache_dir()
        .expect("expected valid cache dir")
        .join(APPLICATION_DIR_NAME)
}

/// State directory handed to the Logos backend (`BackendConfig`): the
/// private logoscore instance (daemon config, module artifacts state,
/// tokens) lives under here unless overridden by `[logos] instance_dir`.
pub fn logos_instance_dir() -> PathBuf {
    data_dir().join("logos")
}

/// Checks if a portable dir is explicitly set or if a config file
/// exists in the same directory as the executable.
/// If so, it'll use that directory for both config & data dirs.
fn portable_dir() -> Option<PathBuf> {
    if let Some(path) = env::var_os("FRIGICOM_PORTABLE_DIR") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return Some(path);
        } else {
            panic!("Given portable directory isn't valid!");
        }
    }

    let exe = env::current_exe().ok()?;
    let dir = exe.parent()?;

    dir.join(CONFIG_FILE_NAME)
        .is_file()
        .then(|| dir.to_path_buf())
}

fn platform_specific_config_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        // Priority order for config directory on macOS:
        // 1. XDG config dir (~/.config/frigicom)
        // 2. User config directory (~/Library/Application Support/frigicom)
        xdg_config_dir().unwrap_or_else(|| {
            dirs_next::config_dir()
                .expect("expected valid config dir")
                .join(APPLICATION_DIR_NAME)
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        dirs_next::config_dir()
            .expect("expected valid config dir")
            .join(APPLICATION_DIR_NAME)
    }
}

#[cfg(target_os = "macos")]
fn xdg_config_dir() -> Option<PathBuf> {
    let config_path = xdg::BaseDirectories::new().config_home?;
    let app_config_dir = config_path.join(APPLICATION_DIR_NAME);

    // if the config file exists, use the xdg config dir
    app_config_dir
        .join(CONFIG_FILE_NAME)
        .is_file()
        .then_some(app_config_dir)
}
