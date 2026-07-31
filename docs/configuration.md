# Configuration

Frigicom uses a TOML file for configuration called `config.toml`.  The specification for the configuration file format ([TOML](https://toml.io/)) can be found at [https://toml.io/](https://toml.io/).

A default file is created in the [configuration directory](#directory) when you launch Frigicom for the first time.

::: tip
Most configuration changes can be applied by reloading the configuration file from the sidebar menu, [keyboard shortcut](./configuration/keyboard.md), or the command bar
:::

The backend is configured in the [`[logos]` section](./configuration/logos.md) — that is where the daemon, the modules directory, and the mock driver are set.

## Directory

The location of the configuration directory depends on your system:

* Windows: `%AppData%\frigicom`
* Mac: `~/Library/Application Support/frigicom` or `$HOME/.config/frigicom`
* Linux: `$XDG_CONFIG_HOME/frigicom` or `$HOME/.config/frigicom`

Placing a `config.toml` next to the executable, or setting `FRIGICOM_PORTABLE_DIR`, switches to [portable mode](./guides/portable-mode.md) and uses that directory for both config and data.
