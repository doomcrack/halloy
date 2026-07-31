use std::env;
use std::path::PathBuf;

// Linking model: the lp_* symbols live in liblogos_protocol.{dylib,so}
// (install name `@rpath/liblogos_protocol.dylib` on macOS), shipped by the
// logos-protocol nix package under `lib/`. When LOGOS_PROTOCOL_ROOT (or
// LOGOS_PROTOCOL_LIB_DIR) is set, we emit link directives plus an rpath so
// this crate's own test binaries run unaided; the `lp_available` cfg gates
// tests that require the library at runtime. When neither env var is set the
// crate still compiles as an rlib with unresolved symbols — only final
// binaries that enable the `ffi` path will fail to link, with a pointer to
// the dev shell.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(lp_available)");
    println!("cargo:rerun-if-env-changed=LOGOS_PROTOCOL_ROOT");
    println!("cargo:rerun-if-env-changed=LOGOS_PROTOCOL_LIB_DIR");

    let lib_dir = env::var("LOGOS_PROTOCOL_LIB_DIR")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            env::var("LOGOS_PROTOCOL_ROOT")
                .ok()
                .map(|root| PathBuf::from(root).join("lib"))
        });

    match lib_dir {
        Some(dir) if dir.is_dir() => {
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=dylib=logos_protocol");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
            println!("cargo:rustc-cfg=lp_available");
        }
        Some(dir) => {
            println!(
                "cargo:warning=LOGOS_PROTOCOL_ROOT points at {} but it has no lib dir; \
                 lp_* symbols will be unresolved (enter the dev shell)",
                dir.display()
            );
        }
        None => {
            println!(
                "cargo:warning=LOGOS_PROTOCOL_ROOT not set; lp_* symbols will be \
                 unresolved at final link (enter the dev shell or run with \
                 --no-default-features where supported)"
            );
        }
    }
}
