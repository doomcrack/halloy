# Logos

The `[logos]` section controls the backend: Frigicom runs a **private
`logoscore` daemon** and talks to `chat_module` through it. The daemon is
started when the app starts and shut down when it exits, so there is no
server list and nothing to connect to by hand.

```toml
[logos]
delivery_preset = "logos.dev"
```

::: warning
Identity is ephemeral upstream: `chat_module` mints a new address on every
`init` and its persistence is compiled off, so conversations and messages do
not survive a restart.
:::

## `delivery_preset`

Network preset passed to `chat_module.init()`.

```toml
# Type: string
# Values: any preset the delivery module accepts, eg. "logos.dev"
#         (public dev fleet, cluster 2) or "logos.test" (test fleet)
# Default: "logos.dev"

[logos]
delivery_preset = "logos.dev"
```

## `daemon_path`

Absolute path to the `logoscore` binary. Overrides the automatic lookup.

When unset, the binary is looked up in this order:

1. the `LOGOSCORE_BIN` environment variable (what the dev shell exports),
2. the copy a packaged build ships beside the executable
   (`logos/bin/logoscore`, or the same under `Contents/Resources` on macOS),
3. `PATH`.

```toml
# Type: string (path)
# Values: absolute path to the logoscore executable
# Default: not set (env → bundled → PATH)

[logos]
daemon_path = "/path/to/bin/logoscore"
```

## `modules_dir`

Directory holding the staged module artifacts (chat, delivery, and
capability modules). Overrides the automatic lookup, which is the same
order as `daemon_path` minus the `PATH` step: `LOGOS_MODULES_DIR`, then
`logos/modules` beside the executable.

```toml
# Type: string (path)
# Values: absolute path to a modules directory
# Default: not set (env → bundled)

[logos]
modules_dir = "/path/to/modules-live"
```

## `instance_dir`

Where the private logoscore instance keeps its daemon config, tokens, and
module state.

```toml
# Type: string (path)
# Values: absolute path to a writable directory
# Default: `<data dir>/logos`

[logos]
instance_dir = "/path/to/instance"
```

See the [configuration directory](../configuration.md#directory) for where
`<data dir>` lives on each platform.

## `installation_name`

Label reported for this installation. Purely cosmetic.

```toml
# Type: string
# Values: any string
# Default: not set

[logos]
installation_name = "my laptop"
```

## `use_wildcard_watch`

Subscribe to chat events with a single wildcard `watchModuleEvents` call
instead of seven explicit ones.

::: warning
The wildcard leg of the QtRO relay is unverified against the pinned daemon.
Leave this off unless you have tested it.
:::

```toml
# Type: boolean
# Values: true, false
# Default: false

[logos]
use_wildcard_watch = true
```

## `mock`

Drive the UI from a scripted in-process mock instead of a real daemon. No
`logoscore`, no modules, and no network — useful for UI work and the only
backend a build without the `live` feature can reach.

```toml
# Type: boolean
# Values: true, false
# Default: false

[logos]
mock = true
```

## Build feature and environment

The FFI transport that talks to a real daemon is behind the non-default
`live` cargo feature, because it links `liblogos_protocol` and needs
`LOGOS_PROTOCOL_ROOT` at build time. A build without `live` fails with
`FfiUnavailable` unless `mock = true`.

```sh
# real backend
. scripts/dev-env.sh     # or: nix develop
cargo run --features live

# UI only, no daemon or dylib required
cargo run               # with `mock = true` in config.toml
```

Both `scripts/dev-env.sh` and the flake's dev shell export the same three
variables:

| Variable               | Meaning                                                                  |
| ---------------------- | ------------------------------------------------------------------------ |
| `LOGOS_PROTOCOL_ROOT`  | Prefix holding `lib/liblogos_protocol.*` and its headers; needed to build and link the `live` feature. |
| `LOGOSCORE_BIN`        | Path to the `logoscore` executable, used when `daemon_path` is unset.    |
| `LOGOS_MODULES_DIR`    | Staged modules directory, used when `modules_dir` is unset. It must hold all three of `chat_module`, `delivery_module`, and `capability_module`; no single upstream output stages them together, so this points at a merged tree. |

They also put `$LOGOS_PROTOCOL_ROOT/lib` on `DYLD_LIBRARY_PATH` (macOS) or
`LD_LIBRARY_PATH`.

See [`logos/README.md`](https://github.com/doomcrack/halloy/tree/main/logos)
in the repository for the backend architecture and the threading rules.
