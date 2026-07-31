//! Reading the auth token the daemon mints for its own clients.
//!
//! Ground truth, established live against the pinned logoscore rev
//! (2026-07-30, `tests/live.rs`): the `ModuleProxy` auth gate accepts ONLY
//! the bootstrap token the daemon writes during startup —
//! `<configDir>/client/<token_file>`, where `token_file` comes from
//! `<configDir>/client/config.json` and defaults to `auto.json` (the daemon
//! mirrors it at `<configDir>/daemon/tokens/auto.json`). A token minted
//! with `issue-token --name <name>` is registered in `daemon/tokens.json`
//! yet every call carrying it is refused with
//! `ModuleProxy: rejecting unauthorized call ... auth token not
//! recognized` — with or without `--local-only`, issued before or after the
//! daemon started, over TCP or LocalSocket. Only the bootstrap token gets a
//! reply. The daemon rotates it on every start, which is exactly right for
//! a private, app-owned daemon; nothing needs issuing.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use tokio::time::Instant;

use crate::DaemonError;

const CLIENT_DIR: &str = "client";
const CLIENT_CONFIG: &str = "config.json";
const DEFAULT_TOKEN_FILE: &str = "auto.json";
const POLL_INTERVAL: Duration = Duration::from_millis(100);

// `<configDir>/client/config.json`, written by the daemon for its own
// clients: `{"version":2,"instance_id":...,"token_file":"auto.json",
// "daemon":{...}}`.
#[derive(Deserialize)]
struct ClientConfig {
    #[serde(default)]
    token_file: Option<String>,
}

// Raw token file shape: `{"version":1,"name":...,"token":"<raw>",
// "issued_at":...}`.
#[derive(Deserialize)]
struct TokenFile {
    token: String,
}

/// Reads the daemon's bootstrap client token, polling until it lands.
/// `config.json` and the token file are written while the daemon comes up,
/// so a caller that raced ahead of them retries rather than failing.
pub async fn read_bootstrap_token(
    config_dir: &Path,
    within: Duration,
) -> Result<String, DaemonError> {
    let deadline = Instant::now() + within;

    loop {
        let reason = match read_bootstrap(config_dir) {
            Ok(token) => return Ok(token),
            Err(reason) => reason,
        };

        if Instant::now() >= deadline {
            return Err(DaemonError::Token(format!(
                "no bootstrap token under {} within {within:?}: {reason}",
                config_dir.join(CLIENT_DIR).display()
            )));
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One attempt: resolve the token file name from `client/config.json` (a
/// missing or unreadable config means the daemon has not written it yet —
/// try the default name, which is what it always picks) and read the raw
/// value out of it.
fn read_bootstrap(config_dir: &Path) -> Result<String, String> {
    let client_dir = config_dir.join(CLIENT_DIR);
    let name = std::fs::read_to_string(client_dir.join(CLIENT_CONFIG))
        .ok()
        .and_then(|contents| {
            serde_json::from_str::<ClientConfig>(&contents).ok()
        })
        .and_then(|config| config.token_file)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| DEFAULT_TOKEN_FILE.to_owned());

    read_token_file(&client_dir.join(name))
}

fn read_token_file(path: &Path) -> Result<String, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;

    let file: TokenFile = serde_json::from_str(&contents)
        .map_err(|error| format!("{}: {error}", path.display()))?;

    if file.token.is_empty() {
        return Err(format!("{}: empty token value", path.display()));
    }

    Ok(file.token)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), contents).unwrap();
    }

    #[tokio::test]
    async fn reads_the_token_file_named_by_the_client_config() {
        let root = tempfile::tempdir().unwrap();
        let client = root.path().join("client");
        write(
            &client,
            "config.json",
            r#"{"version":2,"token_file":"named.json"}"#,
        );
        write(
            &client,
            "named.json",
            r#"{"version":1,"name":"auto","token":"raw-secret"}"#,
        );

        let token =
            read_bootstrap_token(root.path(), Duration::from_millis(100))
                .await
                .unwrap();

        assert_eq!(token, "raw-secret");
    }

    #[tokio::test]
    async fn falls_back_to_auto_json_without_a_client_config() {
        let root = tempfile::tempdir().unwrap();
        write(
            &root.path().join("client"),
            "auto.json",
            r#"{"version":1,"name":"auto","token":"bootstrap"}"#,
        );

        let token =
            read_bootstrap_token(root.path(), Duration::from_millis(100))
                .await
                .unwrap();

        assert_eq!(token, "bootstrap");
    }

    #[tokio::test]
    async fn waits_for_a_token_the_daemon_has_not_written_yet() {
        let root = tempfile::tempdir().unwrap();
        let client = root.path().join("client");
        let writer = client.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            write(
                &writer,
                "auto.json",
                r#"{"version":1,"name":"auto","token":"late"}"#,
            );
        });

        let token = read_bootstrap_token(root.path(), Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(token, "late");
    }

    #[tokio::test]
    async fn reports_missing_empty_and_malformed_tokens() {
        let root = tempfile::tempdir().unwrap();
        let wait = Duration::from_millis(50);

        let error = read_bootstrap_token(root.path(), wait).await.unwrap_err();
        assert!(matches!(error, DaemonError::Token(_)), "{error}");

        let client = root.path().join("client");
        write(&client, "auto.json", r#"{"version":1,"token":""}"#);
        assert!(read_bootstrap_token(root.path(), wait).await.is_err());

        write(&client, "auto.json", "not json");
        assert!(read_bootstrap_token(root.path(), wait).await.is_err());
    }
}
