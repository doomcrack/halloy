# Logs

Customize what is written to Frigicom in-application and file logs.

## `pane_level`

The least urgent (most verbose) log level to record to the Logs pane.
E.g. a `pane_level` setting of `"info"` will record all `ERROR`, `WARN`, and `INFO` messages to the Logs pane.
The Logs pane will only contain log messages since Frigicom was launched.  Log messages that are not recorded to the Logs pane may still be found in log files.

```toml
# Type: string
# Values: "off", "error", "warn", "info", "debug", "trace"
# Default: "info"

[logs]
pane_level = "info"
```

## Files

Log files are named based on the Frigicom launch time, with the [strftime](https://pubs.opengroup.org/onlinepubs/007908799/xsh/strftime.html) format `frigicom.%Y-%m-%d-%H-%M-%S.log`.  They can be found in the log file directory:

* Windows: `%AppData%\Roaming\frigicom\logs\`
* Mac: `~/Library/Application Support/frigicom/logs`
* Linux: `$XDG_DATA_HOME/frigicom/logs` or `$HOME/.local/share/frigicom/logs`

## `file_level`

The least urgent (most verbose) log level to record to log files.
E.g. a `file_level` setting of `"debug"` will record all `ERROR`, `WARN`, `INFO`, and `DEBUG` messages to the log file.

::: warning
Changes to file_level require an application restart to take effect.
:::

```toml
# Type: string
# Values: "off", "error", "warn", "info", "debug", "trace"
# Default: "debug"

[logs]
file_level = "debug"
```

## `max_file_count`

The number of log files to keep in the [log file directory](#files).

```toml
# Type: non-negative integer
# Values: any non-negative integer
# Default: 4

[logs]
max_file_count = 0
```

### `file_timestamp`

Time zone to use for Frigicom-provided timestamps in the file logs and their file names.  `"local"` uses the system's local timezone, and `"utc"` uses the UTC timezone (may be more difficult to understand, but stable if the system changes timezones).

```toml
# Type: string
# Values: "local", "utc"
# Default: "local"

[logs]
file_timestamp = "utc"
```
