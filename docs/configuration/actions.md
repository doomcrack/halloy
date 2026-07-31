# Actions

## `buffer`

Actions inside a conversation buffer.

```toml
# Replace the pane when clicking a sender or member name
[actions.buffer]
click_username = { open-direct = "replace-pane" }
```

### `click_username`

What clicking a sender name in the message log does. `open-direct` opens (or
creates) a direct conversation with that address; `insert-address` inserts the
address into the composer instead.

```toml
# Type: string or object
# Values: { open-direct = "new-pane" }, { open-direct = "replace-pane" }, { open-direct = "new-window" }, "insert-address", "noop"
# Default: { open-direct = "new-pane" }

[actions.buffer]
click_username = "insert-address"
```

::: info
The IRC-era names `click_nickname`, `open-query`, `insert-nickname`, and
`no-action` are still accepted as aliases.
:::

### `open_internal`

Where an [internal buffer](/guides/internal-buffers) opens.

```toml
# Type: string
# Values: "new-pane", "replace-pane", "new-window"
# Default: "new-pane"

[actions.buffer]
open_internal = "replace-pane"
```

### `message_user`

Where the buffer opens when starting a conversation with someone.

```toml
# Type: string
# Values: "new-pane", "replace-pane", "new-window"
# Default: "new-pane"

[actions.buffer]
message_user = "replace-pane"
```

### `only_contract_expanded_message`

When enabled, clicking a message only contracts the message that is currently
expanded.

```toml
# Type: bool
# Values: true, false
# Default: true

[actions.buffer]
only_contract_expanded_message = false
```

### `click_image_url`

What clicking an image URL does.

```toml
# Type: string
# Values: "open-url", "preview"
# Default: "open-url"

[actions.buffer]
click_image_url = "preview"
```

## `member_list`

Actions in the member list of a group conversation.

### `click_username`

What clicking a member does. When unset, falls back to
[`actions.buffer.click_username`](#click_username).

```toml
# Type: string or object
# Values: { open-direct = "new-pane" }, { open-direct = "replace-pane" }, { open-direct = "new-window" }, "insert-address", "noop", not set
# Default: not set

[actions.member_list]
click_username = { open-direct = "new-window" }
```

::: info
The section is also accepted under its old name, `[actions.nicklist]`.
:::

## `notification`

### `default`

What clicking a notification does.

```toml
# Type: string
# Values: "activate-application", "open-buffer"
# Default: "activate-application"

[actions.notification]
default = "open-buffer"
```

### `open_buffer`

Where the buffer opens when `default = "open-buffer"`.

```toml
# Type: string
# Values: "new-pane", "replace-pane", "new-window"
# Default: "new-pane"

[actions.notification]
open_buffer = "new-window"
```

## `sidebar`

### `buffer`

What clicking a conversation in the sidebar does (or closes the buffer if it is
already open).

```toml
# Type: string
# Values: "new-pane", "replace-pane", "new-window"
# Default: "new-pane"

[actions.sidebar]
buffer = "replace-pane"
```

### `focused_buffer`

What clicking the sidebar entry of the already-focused buffer does.

```toml
# Type: string
# Values: "close-pane"
# Default: not set

[actions.sidebar]
focused_buffer = "close-pane"
```

### `cycle`

Whether buffer cycling shortcuts descend into collapsed sidebar sections.

```toml
# Type: string
# Values: "into-collapsed", "skip-collapsed"
# Default: "into-collapsed"

[actions.sidebar]
cycle = "skip-collapsed"
```
