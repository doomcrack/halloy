//! Artifact resolution: explicit overrides → env vars → bundled → PATH.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{Artifact, ArtifactOverrides, Artifacts, LocateError};

const BIN_ENV: &str = "LOGOSCORE_BIN";
const MODULES_ENV: &str = "LOGOS_MODULES_DIR";
const BIN_NAME: &str = "logoscore";
/// Directory a packaged build stages its own daemon and modules in.
const BUNDLE_DIR: &str = "logos";
const BUNDLE_MODULES_DIR: &str = "modules";

pub fn locate(overrides: &ArtifactOverrides) -> Result<Artifacts, LocateError> {
    locate_with(
        overrides,
        env::var_os(BIN_ENV),
        env::var_os(MODULES_ENV),
        env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf)),
        env::var_os("PATH"),
    )
}

// Environment values are passed in explicitly so tests can exercise
// precedence without mutating process-global env vars (unsafe in 2024).
fn locate_with(
    overrides: &ArtifactOverrides,
    env_bin: Option<OsString>,
    env_modules: Option<OsString>,
    exe_dir: Option<PathBuf>,
    path_var: Option<OsString>,
) -> Result<Artifacts, LocateError> {
    // A launched .app inherits no shell environment, so a packaged build
    // has nothing but its own layout to go on. Both are searched: `logos/`
    // beside the executable (portable/linux) and the macOS bundle's
    // `Contents/Resources/logos`.
    let bundles: Vec<PathBuf> = exe_dir
        .iter()
        .flat_map(|dir| {
            [
                dir.join(BUNDLE_DIR),
                dir.join("..").join("Resources").join(BUNDLE_DIR),
            ]
        })
        .collect();

    let mut bin_candidates = Vec::new();
    bin_candidates.extend(overrides.logoscore_bin.clone());
    bin_candidates.extend(env_bin.map(PathBuf::from));
    bin_candidates
        .extend(bundles.iter().map(|root| root.join("bin").join(BIN_NAME)));
    bin_candidates.extend(
        path_var
            .iter()
            .flat_map(env::split_paths)
            .map(|dir| dir.join(BIN_NAME)),
    );

    let mut modules_candidates = Vec::new();
    modules_candidates.extend(overrides.modules_dir.clone());
    modules_candidates.extend(env_modules.map(PathBuf::from));
    modules_candidates
        .extend(bundles.iter().map(|root| root.join(BUNDLE_MODULES_DIR)));

    Ok(Artifacts {
        logoscore_bin: first_existing(
            &bin_candidates,
            Artifact::LogoscoreBinary,
            Path::is_file,
        )?,
        modules_dir: first_existing(
            &modules_candidates,
            Artifact::ModulesDir,
            Path::is_dir,
        )?,
    })
}

fn first_existing(
    candidates: &[PathBuf],
    artifact: Artifact,
    exists: fn(&Path) -> bool,
) -> Result<PathBuf, LocateError> {
    candidates
        .iter()
        .find(|candidate| exists(candidate))
        .cloned()
        .ok_or_else(|| LocateError::NotFound {
            artifact,
            searched: candidates.to_vec(),
        })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    struct Layout {
        _root: tempfile::TempDir,
        override_bin: PathBuf,
        env_bin: PathBuf,
        path_dir: PathBuf,
        path_bin: PathBuf,
        override_modules: PathBuf,
        env_modules: PathBuf,
    }

    fn layout() -> Layout {
        let root = tempfile::tempdir().unwrap();
        let base = root.path();

        for dir in ["override", "env", "path"] {
            fs::create_dir(base.join(dir)).unwrap();
        }

        let layout = Layout {
            override_bin: base.join("override").join(BIN_NAME),
            env_bin: base.join("env").join(BIN_NAME),
            path_dir: base.join("path"),
            path_bin: base.join("path").join(BIN_NAME),
            override_modules: base.join("override-modules"),
            env_modules: base.join("env-modules"),
            _root: root,
        };

        for bin in [&layout.override_bin, &layout.env_bin, &layout.path_bin] {
            fs::write(bin, b"").unwrap();
        }

        for modules in [&layout.override_modules, &layout.env_modules] {
            fs::create_dir(modules).unwrap();
        }

        layout
    }

    fn locate_all(
        layout: &Layout,
        overrides: &ArtifactOverrides,
    ) -> Result<Artifacts, LocateError> {
        locate_with(
            overrides,
            Some(layout.env_bin.clone().into_os_string()),
            Some(layout.env_modules.clone().into_os_string()),
            None,
            Some(layout.path_dir.clone().into_os_string()),
        )
    }

    #[test]
    fn override_beats_env_and_path() {
        let layout = layout();

        let artifacts = locate_all(
            &layout,
            &ArtifactOverrides {
                logoscore_bin: Some(layout.override_bin.clone()),
                modules_dir: Some(layout.override_modules.clone()),
            },
        )
        .unwrap();

        assert_eq!(artifacts.logoscore_bin, layout.override_bin);
        assert_eq!(artifacts.modules_dir, layout.override_modules);
    }

    #[test]
    fn env_beats_path() {
        let layout = layout();

        let artifacts =
            locate_all(&layout, &ArtifactOverrides::default()).unwrap();

        assert_eq!(artifacts.logoscore_bin, layout.env_bin);
        assert_eq!(artifacts.modules_dir, layout.env_modules);
    }

    #[test]
    fn path_is_binary_fallback() {
        let layout = layout();

        let artifacts = locate_with(
            &ArtifactOverrides::default(),
            None,
            Some(layout.env_modules.clone().into_os_string()),
            None,
            Some(layout.path_dir.clone().into_os_string()),
        )
        .unwrap();

        assert_eq!(artifacts.logoscore_bin, layout.path_bin);
    }

    #[test]
    fn missing_override_falls_through() {
        let layout = layout();

        let artifacts = locate_all(
            &layout,
            &ArtifactOverrides {
                logoscore_bin: Some(layout.path_dir.join("no-such-binary")),
                modules_dir: None,
            },
        )
        .unwrap();

        assert_eq!(artifacts.logoscore_bin, layout.env_bin);
    }

    #[test]
    fn not_found_reports_every_searched_location() {
        let layout = layout();
        let missing = layout.path_dir.join("missing");

        let error = locate_with(
            &ArtifactOverrides {
                logoscore_bin: Some(missing.clone()),
                modules_dir: None,
            },
            None,
            None,
            None,
            None,
        )
        .unwrap_err();

        let LocateError::NotFound {
            artifact, searched, ..
        } = error;
        assert_eq!(artifact, Artifact::LogoscoreBinary);
        assert_eq!(searched, vec![missing]);
    }

    // The only source a launched .app has: it inherits no shell
    // environment, so without this both artifacts resolve to nothing and
    // the backend reports ArtifactsMissing on every start.
    #[test]
    fn bundled_layout_beside_the_executable_supplies_both_artifacts() {
        let layout = layout();
        let exe_dir = layout.path_dir.parent().unwrap().join("app");
        let bundle = exe_dir.join(BUNDLE_DIR);
        let bundled_bin = bundle.join("bin").join(BIN_NAME);

        fs::create_dir_all(bundle.join("bin")).unwrap();
        fs::create_dir_all(bundle.join(BUNDLE_MODULES_DIR)).unwrap();
        fs::write(&bundled_bin, b"").unwrap();

        let artifacts = locate_with(
            &ArtifactOverrides::default(),
            None,
            None,
            Some(exe_dir),
            Some(layout.path_dir.clone().into_os_string()),
        )
        .unwrap();

        // Bundled before PATH: a packaged build runs the daemon it shipped
        // with, not whatever the user happens to have installed.
        assert_eq!(artifacts.logoscore_bin, bundled_bin);
        assert_eq!(artifacts.modules_dir, bundle.join(BUNDLE_MODULES_DIR));
    }

    #[test]
    fn macos_bundle_resources_are_searched() {
        let layout = layout();
        let contents = layout.path_dir.parent().unwrap().join("Contents");
        let bundle = contents.join("Resources").join(BUNDLE_DIR);
        let bundled_bin = bundle.join("bin").join(BIN_NAME);

        fs::create_dir_all(contents.join("MacOS")).unwrap();
        fs::create_dir_all(bundle.join("bin")).unwrap();
        fs::create_dir_all(bundle.join(BUNDLE_MODULES_DIR)).unwrap();
        fs::write(&bundled_bin, b"").unwrap();

        let artifacts = locate_with(
            &ArtifactOverrides::default(),
            None,
            None,
            Some(contents.join("MacOS")),
            None,
        )
        .unwrap();

        assert!(artifacts.logoscore_bin.is_file());
        assert!(artifacts.modules_dir.is_dir());
    }

    #[test]
    fn env_beats_the_bundle() {
        let layout = layout();
        let exe_dir = layout.path_dir.parent().unwrap().join("app");
        let bundle = exe_dir.join(BUNDLE_DIR);

        fs::create_dir_all(bundle.join("bin")).unwrap();
        fs::create_dir_all(bundle.join(BUNDLE_MODULES_DIR)).unwrap();
        fs::write(bundle.join("bin").join(BIN_NAME), b"").unwrap();

        let artifacts = locate_with(
            &ArtifactOverrides::default(),
            Some(layout.env_bin.clone().into_os_string()),
            Some(layout.env_modules.clone().into_os_string()),
            Some(exe_dir),
            None,
        )
        .unwrap();

        assert_eq!(artifacts.logoscore_bin, layout.env_bin);
        assert_eq!(artifacts.modules_dir, layout.env_modules);
    }

    #[test]
    fn modules_dir_has_no_path_fallback() {
        let layout = layout();

        let error = locate_with(
            &ArtifactOverrides {
                logoscore_bin: Some(layout.override_bin.clone()),
                modules_dir: None,
            },
            None,
            None,
            None,
            Some(layout.path_dir.clone().into_os_string()),
        )
        .unwrap_err();

        let LocateError::NotFound { artifact, .. } = error;
        assert_eq!(artifact, Artifact::ModulesDir);
    }
}
