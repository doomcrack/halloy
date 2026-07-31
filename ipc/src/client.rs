//! Hands a payload to the instance that already owns a state directory.

use std::io::Write;
use std::path::Path;

use interprocess::local_socket::LocalSocketStream;

use crate::server::socket_name;

/// Sends `payload` to the instance owning `state_dir`. Closing the stream
/// is what ends the read on the other side, so nothing is framed.
pub fn connect_and_send(state_dir: &Path, payload: impl AsRef<[u8]>) -> bool {
    match LocalSocketStream::connect(socket_name(state_dir)) {
        Ok(mut stream) => stream
            .write_all(payload.as_ref())
            .and_then(|()| stream.flush())
            .is_ok(),
        Err(error) => {
            log::warn!("instance handoff failed: {error}");

            false
        }
    }
}
