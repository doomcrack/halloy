#![cfg(lp_available)]

#[test]
fn links_and_reports_compatible_version() {
    let version = logos_sys::version();
    assert!(
        version.split('.').count() >= 3,
        "unexpected protocol version string: {version:?}"
    );
    assert_eq!(logos_sys::abi_ok(), Ok(()), "protocol MAJOR mismatch");
}
