# Frigicom Logos backend

The `logos/` crates replace halloy's IRC stack with the Logos chat backend:
the app runs a private `logoscore` daemon and talks to it through ONE
`lp_client` to `core_service` over plain TCP on loopback, which hosts
`chat_module` (plus the delivery and capability modules it needs).

## Crate map

| Crate          | Path           | Role                                                                                                                                                                  |
| -------------- | -------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `logos-sys`    | `logos/sys`    | Raw `extern "C"` bindings for the lp_* C ABI; `build.rs` links `dylib=logos_protocol` from `$LOGOS_PROTOCOL_ROOT/lib` with rpath baked in; `abi_ok()` startup guard.  |
| `logos-daemon` | `logos/daemon` | logoscore supervision, zero FFI: locate binary/modules, spawn with `port=0` transports, poll status, read resolved ports from `daemon/state.json`, issue token, liveness probe, stop, reap stale daemons. |
| `logos-client` | `logos/client` | The dedicated logos thread owning the lp_client; `Transport` trait (`FfiTransport` behind the default `ffi` feature, `FakeTransport` for tests); `Gateway` with typed core_service calls and the dead-link heuristic. |
| `logos-chat`   | `logos/chat`   | Typed chat_module contract (13 methods, 7 events), `ChatClient`, and `session::run()` — the state machine the UI consumes as an iced subscription.                    |

Dependency direction: `logos-chat` → `logos-client` → `logos-sys`, and
`logos-chat` → `logos-daemon`. The `data` crate depends on `logos-chat`;
nothing under `logos/` depends on `data`.

## Threading rules

- **One logos thread.** Each lp_client is created, subscribed, invoked, and
  destroyed on a single dedicated thread. Cross-thread marshaling deadlocks
  without a Qt event loop.
- **Sync only.** Use synchronous `lp_invoke` exclusively — async completions
  are silently dropped in a Qt-free process.
- **Events arrive on the Asio thread.** `lp_subscribe("module_event")`
  callbacks fire on the library's Asio IO thread: the trampoline only copies
  the payload into an unbounded channel and returns. Never block there and
  never call back into lp_* from it.
- **Subscribe before watching.** Arm the `module_event` subscription before
  any `watchModuleEvents` call to avoid the relay race.
- **Dead links don't error.** After daemon death a cached handle returns
  `LP_OK`/`"null"` forever — treat that as a dead link and rebuild the client
  and daemon; never trust a stale handle.
- **One live backend per process.** `lp_token_save`/`lp_token_get` are a
  process-wide store keyed by TARGET MODULE name, and the library looks the
  token up on every call, so a second `lp_client` for `core_service` takes
  over the first one's token slot. Two sessions in one process then present
  one daemon's token to both daemons, and the loser's calls come back
  `LP_OK`/`"null"` — indistinguishable from a dead link. Two backends means
  two processes; `logos/chat/tests/two_instance.rs` re-executes its own test
  binary for exactly this reason.

## Developer environment

Source the dev env (or use `nix develop`; the flake exports the same
variables from flake-built packages — the rust toolchain itself comes from
rustup, since the flake's nixpkgs pin is too old for this workspace):

```sh
. scripts/dev-env.sh
```

This exports `LOGOS_PROTOCOL_ROOT`, `LOGOSCORE_BIN`, `LOGOS_MODULES_DIR`, and
puts `$LOGOS_PROTOCOL_ROOT/lib` on `DYLD_LIBRARY_PATH`/`LD_LIBRARY_PATH`.

Nothing in it is hardcoded to one machine. Paths are derived from the
checkout, so any of these overrides the defaults:

| Variable | Default |
| --- | --- |
| `FRIGICOM_ARTIFACTS` | `<repo>/../.gcroots` — the nix GC roots and staged module trees |
| `LOGOS_PROTOCOL_ROOT` | newest `$FRIGICOM_ARTIFACTS/logos-protocol-lib-*`, resolved to its store path |
| `LOGOSCORE_BIN` | `$FRIGICOM_ARTIFACTS/logos-logoscore-cli/bin/logoscore` |
| `LOGOS_MODULES_DIR` | first existing staged tree (below) |

`nix develop` sets `LOGOS_PROTOCOL_ROOT` and `LOGOSCORE_BIN` from flake-built
packages and then sources this same script for the rest, so the module
search and the linker path behave identically either way.

### The module set behind `LOGOS_MODULES_DIR`

The daemon wants `chat_module`, `delivery_module` and `capability_module` in
ONE directory and no upstream output stages them together, so
`LOGOS_MODULES_DIR` points at a merged tree. `scripts/dev-env.sh` prefers
`$FRIGICOM_ARTIFACTS/modules-live-v3`, falls back to `modules-live-v2` then
`modules-live`, then the single-module dirs.

`modules-live-v3` is the set to develop against. It pairs a `chat_module`
**0.2.1** built from logos-chat-module's `fix/delivery-config-and-online-state`
(the same three fixes, rebased onto the 0.2.1 tag) with the genuine
`delivery_module` 0.1.3 — byte-identical to the one in `modules-live-v2`.
On it `chat.init({delivery_preset: "logos.dev"})` reaches the fleet: a real
Waku node with an ENR, discv5, and dialed fleet peers, under four seconds
from init to `Phase::Online`, and `tests/two_instance.rs` completes the
round trip through the key-package registry.

`modules-live-v2` is the same three fixes on chat_module 0.2.0, kept as a
fallback.

Three things about these sets are worth carrying into any debugging session:

- **`init` changed shape at 0.2.1, and neither generation rejects the
  other's argument.** 0.2.1 takes a `ChatConfig` record where 0.2.0 took the
  preset as a bare string. A 0.2.1 module cannot parse a bare string as a
  record; a 0.2.0 module reads a record as an empty string. Both then fall
  back to their own `logos.dev` default, so a mismatch never lands you on
  the wrong network — it silently ignores the preset you asked for.
  `logos-chat` sends the record, so a `logos.test` run needs v3; on v2 it
  would quietly go to logos.dev instead.

- **Manifest versions do not identify a build.** `delivery_module`'s
  `manifest.json` carries the same shape of `version` string across
  materially different libraries, so a matching stamp says nothing about
  which delivery you loaded — the previous `modules-live` tree stamps a
  version too and its node never starts. Compare the store path (or the
  `liblogosdelivery.dylib` size) when it matters, not the manifest, and if
  a run behaves unexpectedly check which tree `LOGOS_MODULES_DIR` resolved
  to before anything else.
- **`Online` now implies a started node.** It used to be chat_module's
  unverified claim: a `createNode` or `start` the delivery module ran and
  rejected came back as a successful call carrying a failure envelope, so
  the module went `online` over a node that did not exist. The patched
  chat_module reads that envelope before advancing, so a rejected step
  leaves `delivery_state` in `error` with the reason and stops the chain.
  On v3 and v2 `Phase::Online` is therefore load-bearing, and `tests/live.rs`
  asserts it: reaching Online with `createNode callback error` in the daemon
  log is a failure, not a note. On the older tree the claim is still just a
  claim — read the log there.
- **0.2.1 keeps its own log.** The chat core writes a `tracing` log into the
  instance directory the host assigned (`get_log_path()` names the file), at
  a level the `ChatConfig.log_level` field sets and `RUST_LOG` overrides. It
  records the transitions — the preset it joined, delivery state changes,
  message sizes — but never message text. `logos-chat` does not read the
  path today; when a live run goes wrong on v3, that file is the second
  place to look after the daemon log.

Unit tests need no dylib and no daemon:

```sh
cargo test -p logos-daemon
cargo test -p logos-client --no-default-features
cargo test -p logos-chat --no-default-features
```

With the dev env sourced (dylib available). `ffi` is not a default feature
of `logos-chat`, so the FFI paths only build when it is asked for:

```sh
cargo test -p logos-sys      # link smoke test: lp_protocol_version()
cargo test -p logos-client
cargo test -p logos-chat --features ffi
```

Live integration test (real daemon, real network; self-skips when
`LOGOS_LIVE_TESTS` is unset — the whole file is `#[cfg(feature = "ffi")]`):

```sh
LOGOS_LIVE_TESTS=1 cargo test -p logos-chat --features ffi --test live \
    -- --nocapture
```

Two-instance exchange (two daemons, two delivery nodes, a direct
conversation created on one and received on the other). It needs the fleet
AND the key-package registry, so it asks for a second opt-in, and it
re-executes its own test binary for the second instance — see "One live
backend per process" above:

```sh
LOGOS_LIVE_TESTS=1 LOGOS_LIVE_NETWORK=1 cargo test -p logos-chat \
    --features ffi --test two_instance -- --nocapture
```

## Daemon recipe

What `logos-daemon`'s supervisor runs under the hood:

```sh
"$LOGOSCORE_BIN" -D -m "$LOGOS_MODULES_DIR" \
  --module-transport core_service=tcp,host=127.0.0.1,port=0 \
  --module-transport capability_module=tcp,host=127.0.0.1,port=0
```

Then read the resolved ports from `<configDir>/daemon/state.json` and the
auth token from `<configDir>/client/<token_file>` (`auto.json` by default),
and `load-module chat_module` on every start (the daemon starts clean).
Daemon lifetime is tied to app lifetime: there is no unwatch, and
re-watching duplicates event forwarders — spawn a fresh daemon on app start
and `core_service.shutdown` it on exit.

**Nothing is issued.** On the pinned logoscore rev the `ModuleProxy` auth
gate accepts ONLY the bootstrap token the daemon mints for itself during
startup. A token from `issue-token --name <name>` lands in
`daemon/tokens.json` and is then refused on every call — `auth token not
recognized` — with or without `--local-only`, over TCP or LocalSocket. The
daemon rotates the bootstrap token on every start, which is exactly what a
private, app-owned daemon wants; see `logos/daemon/src/token.rs`.
