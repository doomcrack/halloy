//! Parsing of `<configDir>/daemon/state.json` — specifically the resolved
//! per-module transport lists that carry the actually-bound TCP ports.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::{DaemonError, Ports};

#[derive(Deserialize)]
struct StateJson {
    #[serde(default)]
    pid: i64,
    resolved: Resolved,
}

#[derive(Deserialize)]
struct Resolved {
    #[serde(default)]
    modules: HashMap<String, Module>,
}

#[derive(Deserialize)]
struct Module {
    #[serde(default)]
    transports: Vec<Transport>,
}

#[derive(Deserialize)]
struct Transport {
    protocol: String,
    #[serde(default)]
    port: u16,
}

#[derive(Deserialize)]
struct StatePid {
    pid: i64,
}

/// Reads the bound loopback TCP ports for core_service and
/// capability_module from a state.json. Errors when the file is missing,
/// unparsable, or either module lacks a tcp transport with a non-zero
/// port.
pub fn read_ports(state_json_path: &Path) -> Result<Ports, DaemonError> {
    let state: StateJson = parse(state_json_path)?;

    ports_of(&state, state_json_path)
}

/// [`read_ports`], but only trusting a state.json written by the expected
/// daemon process. A crashed prior run leaves its state.json behind (only a
/// clean exit removes it), so during startup the file may still be the DEAD
/// daemon's for a while; pid and ports are parsed from one read so the file
/// cannot change between the two checks. `-D` runs in the foreground — the
/// recorded pid IS the spawned child's.
pub(crate) fn read_ports_for_pid(
    state_json_path: &Path,
    expected_pid: u32,
) -> Result<Ports, DaemonError> {
    let state: StateJson = parse(state_json_path)?;

    if state.pid != i64::from(expected_pid) {
        return Err(DaemonError::State(format!(
            "{}: names pid {} (expected the spawned daemon, pid \
             {expected_pid})",
            state_json_path.display(),
            state.pid
        )));
    }

    ports_of(&state, state_json_path)
}

pub(crate) fn read_pid(state_json_path: &Path) -> Result<i64, DaemonError> {
    parse::<StatePid>(state_json_path).map(|state| state.pid)
}

pub(crate) fn path_in(config_dir: &Path) -> PathBuf {
    config_dir.join("daemon").join("state.json")
}

fn parse<T: DeserializeOwned>(path: &Path) -> Result<T, DaemonError> {
    let contents = std::fs::read_to_string(path).map_err(|error| {
        DaemonError::State(format!("{}: {error}", path.display()))
    })?;

    serde_json::from_str(&contents).map_err(|error| {
        DaemonError::State(format!("{}: {error}", path.display()))
    })
}

fn ports_of(
    state: &StateJson,
    state_json_path: &Path,
) -> Result<Ports, DaemonError> {
    Ok(Ports {
        core_service: tcp_port(state, state_json_path, "core_service")?,
        capability_module: tcp_port(
            state,
            state_json_path,
            "capability_module",
        )?,
    })
}

fn tcp_port(
    state: &StateJson,
    path: &Path,
    module: &str,
) -> Result<u16, DaemonError> {
    let transport = state
        .resolved
        .modules
        .get(module)
        .and_then(|entry| {
            entry
                .transports
                .iter()
                .find(|transport| transport.protocol == "tcp")
        })
        .ok_or_else(|| {
            DaemonError::State(format!(
                "{}: {module} has no tcp transport",
                path.display()
            ))
        })?;

    if transport.port == 0 {
        return Err(DaemonError::State(format!(
            "{}: {module} tcp port is 0 (not yet resolved)",
            path.display()
        )));
    }

    Ok(transport.port)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    // Mirrors the documented state.json schema (docs/project.md): a local
    // transport is always implicitly prepended, tcp entries follow.
    const FIXTURE: &str = r#"{
        "version": 2,
        "instance_id": "a3f1c8d20b4e",
        "pid": 12345,
        "started_at": "2026-03-23T14:00:00Z",
        "config_source": "cli",
        "resolved": {
            "modules_dirs": ["/path/to/modules"],
            "modules": {
                "core_service": {
                    "transports": [
                        { "protocol": "local" },
                        { "protocol": "tcp", "host": "127.0.0.1", "port": 52001, "codec": "json" }
                    ]
                },
                "capability_module": {
                    "transports": [
                        { "protocol": "local" },
                        { "protocol": "tcp", "host": "127.0.0.1", "port": 52002, "codec": "json" }
                    ]
                }
            },
            "insecure_tcp": false
        }
    }"#;

    fn write_fixture(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, contents).unwrap();
        (dir, path)
    }

    #[test]
    fn reads_both_tcp_ports() {
        let (_dir, path) = write_fixture(FIXTURE);

        let ports = read_ports(&path).unwrap();

        assert_eq!(ports.core_service, 52001);
        assert_eq!(ports.capability_module, 52002);
    }

    #[test]
    fn reads_pid() {
        let (_dir, path) = write_fixture(FIXTURE);

        assert_eq!(read_pid(&path).unwrap(), 12345);
    }

    #[test]
    fn verified_read_accepts_the_expected_pid_only() {
        let (_dir, path) = write_fixture(FIXTURE);

        let ports = read_ports_for_pid(&path, 12345).unwrap();
        assert_eq!(ports.core_service, 52001);

        let error = read_ports_for_pid(&path, 99999).unwrap_err();
        assert!(error.to_string().contains("names pid 12345"));
    }

    #[test]
    fn errors_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();

        let error = read_ports(&dir.path().join("state.json")).unwrap_err();

        assert!(matches!(error, DaemonError::State(_)));
    }

    #[test]
    fn errors_on_unparsable_json() {
        let (_dir, path) = write_fixture("{ not json");

        assert!(read_ports(&path).is_err());
    }

    #[test]
    fn errors_on_unresolved_zero_port() {
        let (_dir, path) = write_fixture(&FIXTURE.replace("52001", "0"));

        let error = read_ports(&path).unwrap_err();

        assert!(error.to_string().contains("core_service"));
        assert!(error.to_string().contains("port is 0"));
    }

    #[test]
    fn errors_when_module_lacks_tcp_transport() {
        let fixture = FIXTURE.replace(
            r#"{ "protocol": "tcp", "host": "127.0.0.1", "port": 52002, "codec": "json" }"#,
            r#"{ "protocol": "local" }"#,
        );
        let (_dir, path) = write_fixture(&fixture);

        let error = read_ports(&path).unwrap_err();

        assert!(error.to_string().contains("capability_module"));
        assert!(error.to_string().contains("no tcp transport"));
    }

    #[test]
    fn errors_when_module_is_missing() {
        let fixture = FIXTURE.replace("capability_module", "some_other_module");
        let (_dir, path) = write_fixture(&fixture);

        assert!(read_ports(&path).is_err());
    }
}
