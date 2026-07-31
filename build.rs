use std::env;
use std::path::PathBuf;

// `logos-sys` emits the link search path and `-l logos_protocol` (those
// propagate down the dependency chain), but `cargo:rustc-link-arg` only
// reaches the emitting package's own targets — so the runtime search path
// for `@rpath/liblogos_protocol.dylib` has to be emitted here, against the
// binary. Only relevant with the `live` feature; when it is off there is
// nothing to link and the env var is ignored. Missing paths warn instead of
// failing: the diagnostic that matters is the linker's, not ours.
fn main() {
    #[cfg(windows)]
    {
        let _ = embed_resource::compile(
            "assets/windows/halloy.rc",
            embed_resource::NONE,
        );
        windows_exe_info::versioninfo::link_cargo_env();
    }

    println!("cargo:rerun-if-env-changed=LOGOS_PROTOCOL_ROOT");
    println!("cargo:rerun-if-env-changed=LOGOS_PROTOCOL_LIB_DIR");

    if env::var_os("CARGO_FEATURE_LIVE").is_none() {
        return;
    }

    let lib_dir = env::var("LOGOS_PROTOCOL_LIB_DIR")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            env::var("LOGOS_PROTOCOL_ROOT")
                .ok()
                .map(|root| PathBuf::from(root).join("lib"))
        });

    // A packaged .app cannot resolve the absolute store path below on any
    // machine but the build host, so macOS binaries also carry the rpath a
    // bundled copy would live at. Staging `liblogos_protocol.dylib` into
    // `Contents/Frameworks` (and signing it before the app) is packaging
    // work the scripts do not do yet — the packaging profile builds
    // without `live`, so nothing links the dylib today.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!(
            "cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks"
        );
    }

    match lib_dir {
        Some(dir) if dir.is_dir() => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        }
        Some(dir) => {
            println!(
                "cargo:warning=LOGOS_PROTOCOL_ROOT points at {} but it has no \
                 lib dir; the live backend will not link (enter the dev shell)",
                dir.display()
            );
        }
        None => {
            println!(
                "cargo:warning=LOGOS_PROTOCOL_ROOT not set; the live backend \
                 will not link (enter the dev shell, or build with \
                 --no-default-features --features iosevka-font for a \
                 mock-only binary)"
            );
        }
    }
}
