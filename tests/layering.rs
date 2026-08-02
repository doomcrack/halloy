//! The licence boundary, asserted rather than documented.
//!
//! Frigicom is a halloy fork and is GPL-3.0-or-later. The Logos client
//! stack it runs on is not derived from halloy, lives in its own repository
//! (`doomcrack/logos-rs`) and is dual MIT/Apache-2.0. That split only holds
//! while the dependency arrow points one way: the app may depend on the
//! stack, and the stack may never depend on the app.
//!
//! Nothing in the compiler enforces that. A `logos-*` crate that grew a
//! `data` dependency would build perfectly and quietly make GPL code a
//! prerequisite of a permissively licensed crate — the exact confusion the
//! split exists to prevent, and invisible in review because it is one line
//! of a manifest. So it is checked here, over the resolved graph rather
//! than over the manifests, which is what catches it arriving through a
//! transitive edge or an optional feature.

use std::process::Command;

use serde_json::Value;

/// Crates in this repository, inherited from halloy and GPL-3.0-or-later.
const GPL_CRATES: &[&str] = &["frigicom", "data", "ipc"];

/// What the separated stack must declare. Catches the other direction of
/// the same mistake: a crate re-inheriting the fork's licence.
const PERMISSIVE: &str = "MIT OR Apache-2.0";

fn metadata() -> Value {
    // `--all-features` so the `live` path's optional edges are in the graph
    // too; metadata resolves, it does not build, so no dylib is needed.
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--all-features"])
        .output()
        .expect("cargo metadata");

    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    serde_json::from_slice(&output.stdout).expect("metadata is json")
}

fn packages(metadata: &Value) -> impl Iterator<Item = &Value> {
    metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .filter(|package| {
            package["name"].as_str().is_some_and(|name| {
                name.starts_with("logos-") || name == "logos-domain"
            })
        })
}

#[test]
fn the_logos_stack_never_depends_on_the_fork() {
    let metadata = metadata();
    let mut found = 0;

    for package in packages(&metadata) {
        let name = package["name"].as_str().unwrap();
        found += 1;

        for dependency in package["dependencies"].as_array().unwrap() {
            let dependency = dependency["name"].as_str().unwrap();

            assert!(
                !GPL_CRATES.contains(&dependency),
                "{name} depends on {dependency}. The Logos stack is \
                 MIT/Apache-2.0 and must not require GPL code — see the \
                 module docs on this file before changing anything."
            );
        }
    }

    // A rename or a dropped dependency would otherwise make this vacuous.
    assert!(
        found >= 5,
        "expected the five logos-* crates in the graph, found {found}"
    );
}

#[test]
fn the_logos_stack_keeps_its_own_licence() {
    let metadata = metadata();

    for package in packages(&metadata) {
        let name = package["name"].as_str().unwrap();

        assert_eq!(
            package["license"].as_str(),
            Some(PERMISSIVE),
            "{name} should be {PERMISSIVE}; a crate that inherits this \
             workspace's GPL is one that has moved back inside the fork"
        );
    }
}
