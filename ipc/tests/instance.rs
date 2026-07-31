//! Roundtrip of the single-instance guard on a throwaway state directory:
//! stale socket recovery, refusal of a second owner, and handoff delivery.

use std::sync::mpsc;
use std::time::Duration;
use std::{env, fs, process, thread};

use futures::StreamExt;

/// One test only: the handoff queue is process wide, so a second test
/// acquiring in parallel would steal it.
#[test]
fn later_instance_hands_over_to_the_owner() {
    let state_dir =
        env::temp_dir().join(format!("frigicom-ipc-{}", process::id()));

    let _ = fs::remove_dir_all(&state_dir);
    fs::create_dir_all(&state_dir).expect("state dir");

    // A file where the socket belongs is what an unclean exit leaves
    // behind; binding must reclaim it instead of refusing to start.
    fs::write(state_dir.join("instance.sock"), []).expect("stale socket");

    assert_eq!(ipc::acquire(&state_dir), ipc::Acquired::Owner);
    assert_eq!(ipc::acquire(&state_dir), ipc::Acquired::AlreadyRunning);

    assert!(ipc::connect_and_send(&state_dir, ipc::FOCUS));

    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let _ = sender.send(futures::executor::block_on(ipc::listen().next()));
    });

    let handoff = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("handoff delivered");

    assert_eq!(handoff.as_deref(), Some(ipc::FOCUS));

    // No owner, no handoff — the caller must not exit believing one ran.
    assert!(!ipc::connect_and_send(
        &state_dir.join("elsewhere"),
        ipc::FOCUS
    ));

    let _ = fs::remove_dir_all(&state_dir);
}
