//! Single-instance guard. The first process to bind the socket of a
//! backend state directory owns it; later ones hand their payload over
//! and exit. Two apps on one state dir would spawn two logoscore
//! daemons, and the second one reaps the first.

use std::collections::hash_map::DefaultHasher;
use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;
use std::{fs, io, thread};

use futures::channel::mpsc;
use futures::stream::{BoxStream, StreamExt};
use interprocess::local_socket::{LocalSocketListener, LocalSocketStream};

/// Whether this process owns the state directory, or another process
/// already does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acquired {
    Owner,
    AlreadyRunning,
    /// The guard could not be established, so the app starts without one:
    /// a socket we cannot bind must never keep the app shut. Ownership is
    /// unknown from here on — the caller has to assume another instance
    /// may be live and keep its hands off shared state (above all, off the
    /// daemon that `daemon/state.json` describes).
    Unguarded,
}

/// Payload a later instance sends when it has nothing to hand over but
/// still wants the running app brought to the front.
pub const FOCUS: &str = "focus";

#[cfg(not(windows))]
const SOCKET_FILE_NAME: &str = "instance.sock";

/// `sockaddr_un` holds 104 bytes on macOS, 108 on Linux.
#[cfg(not(windows))]
const MAX_SOCKET_PATH: usize = 100;

/// Filled in by [`acquire`], drained once by [`listen`].
static INBOX: Mutex<Option<mpsc::UnboundedReceiver<String>>> = Mutex::new(None);

/// Claims `state_dir` for this process. Must run before the backend
/// session starts, otherwise a second daemon reaps the first one.
pub fn acquire(state_dir: &Path) -> Acquired {
    if let Err(error) = fs::create_dir_all(state_dir) {
        log::warn!("instance guard cannot use {state_dir:?}: {error}");

        return Acquired::Unguarded;
    }

    match bind(state_dir) {
        Ok(Some(listener)) => {
            serve(listener);

            Acquired::Owner
        }
        Ok(None) => Acquired::AlreadyRunning,
        Err(error) => {
            // A guard we cannot build must never keep the app shut — but
            // it is reported as unguarded, not owned: the caller cannot
            // treat exclusive state as its own on the strength of a
            // failure.
            log::error!("instance guard unavailable: {error}");

            Acquired::Unguarded
        }
    }
}

/// Payloads handed over by later instances: a url to route, or [`FOCUS`].
/// Yields the queue once; further calls get an inert stream.
pub fn listen() -> BoxStream<'static, String> {
    match INBOX.lock().ok().and_then(|mut inbox| inbox.take()) {
        Some(receiver) => receiver.boxed(),
        None => futures::stream::pending().boxed(),
    }
}

/// Unix keeps the socket next to the state it guards, unless the path is
/// too long for `sockaddr_un` — then the state dir is hashed into a name
/// under the temp dir, which is where Windows named pipes live anyway.
#[cfg(not(windows))]
pub(crate) fn socket_name(state_dir: &Path) -> OsString {
    let path = state_dir.join(SOCKET_FILE_NAME);

    if path.as_os_str().len() < MAX_SOCKET_PATH {
        return path.into_os_string();
    }

    std::env::temp_dir()
        .join(format!("frigicom-{}.sock", digest(state_dir)))
        .into_os_string()
}

#[cfg(windows)]
pub(crate) fn socket_name(state_dir: &Path) -> OsString {
    OsString::from(format!("frigicom-{}", digest(state_dir)))
}

/// Same state dir, same socket, in every process that starts the app.
fn digest(state_dir: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    state_dir.hash(&mut hasher);

    format!("{:016x}", hasher.finish())
}

/// `Ok(None)` when a live instance answered on the socket. A socket file
/// outlives an unclean exit, so only a peer that accepts a connection
/// proves that an instance is actually running.
#[cfg(not(windows))]
fn bind(state_dir: &Path) -> io::Result<Option<LocalSocketListener>> {
    let name = socket_name(state_dir);

    match LocalSocketListener::bind(name.clone()) {
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            if LocalSocketStream::connect(name.clone()).is_ok() {
                return Ok(None);
            }

            fs::remove_file(&name)?;

            LocalSocketListener::bind(name).map(Some)
        }
        result => result.map(Some),
    }
}

#[cfg(windows)]
fn bind(state_dir: &Path) -> io::Result<Option<LocalSocketListener>> {
    let name = socket_name(state_dir);

    if LocalSocketStream::connect(name.clone()).is_ok() {
        return Ok(None);
    }

    LocalSocketListener::bind(name).map(Some)
}

/// Accepts on a plain thread and hands payloads to [`listen`]: the socket is
/// bound before iced starts, so it cannot ride an async runtime. The
/// thread owns the listener for the life of the process.
fn serve(listener: LocalSocketListener) {
    let (sender, receiver) = mpsc::unbounded();

    if let Ok(mut inbox) = INBOX.lock() {
        *inbox = Some(receiver);
    }

    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut connection) = connection else {
                continue;
            };

            let mut payload = String::new();

            if let Err(error) = connection.read_to_string(&mut payload) {
                log::warn!("instance handoff unreadable: {error}");

                continue;
            }

            let payload = payload.trim();

            if !payload.is_empty() {
                let _ = sender.unbounded_send(payload.to_owned());
            }
        }
    });
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::{MAX_SOCKET_PATH, SOCKET_FILE_NAME, socket_name};

    #[test]
    fn a_state_dir_too_deep_for_sockaddr_un_falls_back_to_the_temp_dir() {
        let short = std::path::Path::new("/tmp/frigicom");

        assert_eq!(
            std::path::Path::new(&socket_name(short)),
            short.join(SOCKET_FILE_NAME)
        );

        let deep =
            std::path::Path::new("/tmp").join("d".repeat(MAX_SOCKET_PATH));
        let name = socket_name(&deep);

        assert!(name.len() < MAX_SOCKET_PATH);
        assert_ne!(
            std::path::Path::new(&name),
            deep.join(SOCKET_FILE_NAME).as_path()
        );
        assert_eq!(socket_name(&deep), name);
    }
}
