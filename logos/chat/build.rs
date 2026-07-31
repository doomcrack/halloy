use std::env;
use std::path::PathBuf;

// `cargo:rustc-link-arg` does not propagate across crates, so the rpath
// logos-sys bakes into its own targets never reaches ours. When the `ffi`
// feature links the dylib in, re-emit the rpath here so this crate's test
// binaries run unaided inside the dev shell.
fn main() {
    println!("cargo:rerun-if-env-changed=LOGOS_PROTOCOL_ROOT");
    println!("cargo:rerun-if-env-changed=LOGOS_PROTOCOL_LIB_DIR");

    if env::var_os("CARGO_FEATURE_FFI").is_none() {
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

    if let Some(dir) = lib_dir
        && dir.is_dir()
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
    }
}
