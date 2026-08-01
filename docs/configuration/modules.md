# Modules

Settings for the module monitor — the sidebar group listing the Logos modules
Frigicom runs (delivery, blockchain, and the rest of the staged set), and the
read-only log pane each of them opens.

The module set itself is not configurable. Frigicom ships a fixed set and
asks the daemon about its status; there is no way to add a module here.

## `log_level`

The least urgent (most verbose) level a module log line may carry and still
appear in that module's pane. A `log_level` of `"info"` shows `ERROR`, `WARN`
and `INFO` lines and drops `DEBUG` and `TRACE`.

The default is not cosmetic. A blockchain node logs at `DEBUG` by default, and
a running delivery node has been observed emitting roughly two thousand lines
in a few minutes with 38% of them `DEBUG` — enough to push everything else out
of the pane's scrollback within seconds. Lower it only when chasing a specific
problem.

Lines below the floor are dropped before they are stored; they remain in the
daemon's own log file on disk. Crash lines are raised above every floor except
`"off"`, so a module that dies always says so.

```toml
# Type: string
# Values: "off", "error", "warn", "info", "debug", "trace"
# Default: "info"

[modules]
log_level = "info"
```
