# Check for Update on Launch

Controls whether Frigicom will check the Frigicom repository on launch for a new version of Frigicom.  When a new version is found a dot indicator will appear on the user menu button and a menu item to open the release webpage will be added to the user menu.

This can be useful if you would rather rely on a package manager.

## `check_for_update_on_launch`

::: warning
`check_for_update_on_launch` is a root key, so it must be placed before any section.
:::

```toml
# Type: boolean
# Values: true, false
# Default: true

check_for_update_on_launch = true
```
