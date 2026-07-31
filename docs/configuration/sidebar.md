# Sidebar

Sidebar settings for Frigicom.

## `position`

Where the sidebar is docked.

```toml
# Type: string
# Values: "left", "right", "top", "bottom"
# Default: "left"

[sidebar]
position = "top"
```

## `max_width`

Maximum width of the sidebar, in pixels. Only used for vertical positions.

```toml
# Type: integer
# Values: any non-negative integer
# Default: not set

[sidebar]
max_width = 200
```

## `ordering`

Order of the conversation list. `"recent"` sorts by last activity (newest
first); `"alpha"` sorts by display name.

```toml
# Type: string
# Values: "recent", "alpha"
# Default: "recent"

[sidebar]
ordering = "alpha"
```

## `primary_font_size`

Font size of the conversation name.

```toml
# Type: integer
# Values: any positive integer or not set
# Default: not set (uses the configured font size)

[sidebar]
primary_font_size = 14
```

## `secondary_font_size`

Font size of the secondary line (message preview, timestamps).

```toml
# Type: integer
# Values: any positive integer or not set
# Default: not set (uses the configured font size)

[sidebar]
secondary_font_size = 12
```

## `user_menu`

### `enabled`

Show the overflow menu in the account card.

```toml
# Type: boolean
# Values: true, false
# Default: true

[sidebar.user_menu]
enabled = false
```

## `collapse_button`

### `enabled`

Show the sidebar collapse button.

```toml
# Type: boolean
# Values: true, false
# Default: true

[sidebar.collapse_button]
enabled = false
```

## `internal_buffers` {#internal-buffers}

Which [internal buffers](/guides/internal-buffers) are pinned in the sidebar,
where they sit, and which of them are muted.

```toml
# Type: table
# Values: `position`, `buffers`, and `mute`
# Default: `{ position = "after-conversations", buffers = [], mute = [] }`

[sidebar.internal_buffers]
buffers = ["logs", "config-editor"]
```

### `position`

```toml
# Type: string
# Values: "before-conversations", "after-conversations"
# Default: "after-conversations"

[sidebar.internal_buffers]
position = "before-conversations"
```

::: info
The IRC-era values `before-servers` and `after-servers` are accepted as aliases.
:::

### `buffers`

```toml
# Type: array of strings
# Values: "config-editor", "logs"
# Default: []

[sidebar.internal_buffers]
buffers = ["logs"]
```

### `mute`

Muted internal buffers are hidden unless they have unread messages, or are shown
with the `show_muted_buffers` [keyboard shortcut](/configuration/keyboard).

```toml
# Type: array of strings
# Values: "logs"
# Default: []

[sidebar.internal_buffers]
mute = ["logs"]
```

## `scrollbar`

### `width`

```toml
# Type: integer
# Values: any non-negative integer
# Default: 5

[sidebar.scrollbar]
width = 8
```

### `scroller_width`

```toml
# Type: integer
# Values: any non-negative integer
# Default: 5

[sidebar.scrollbar]
scroller_width = 8
```

## `unread_indicator`

How unread conversations are marked. The whole table can be replaced by the
shorthand strings `"dot"`, `"title"`, or `"none"`.

```toml
# Type: table or string
# Values: table, or "dot" / "title" / "none"
# Default: { title = false, icon = "dot", icon_size = 6, show_on_open_buffers = true }

[sidebar]
unread_indicator = "title"
```

### `title`

Highlight the conversation name when it has unread messages.

```toml
# Type: boolean
# Values: true, false
# Default: false

[sidebar.unread_indicator]
title = true
```

### `icon`

```toml
# Type: string
# Values: "dot", "circle-empty", "dot-circled", "certificate", "asterisk", "speaker", "lightbulb", "star", "none"
# Default: "dot"

[sidebar.unread_indicator]
icon = "star"
```

### `icon_size`

```toml
# Type: integer
# Values: any positive integer
# Default: 6

[sidebar.unread_indicator]
icon_size = 8
```

### `show_on_open_buffers`

Whether the indicator is also shown for conversations that are open in a pane.

```toml
# Type: boolean
# Values: true, false
# Default: true

[sidebar.unread_indicator]
show_on_open_buffers = false
```

## `padding`

### `buffer`

Padding around each sidebar entry: `[vertical, horizontal]`.

```toml
# Type: array
# Values: array of two integers
# Default: [5, 4]

[sidebar.padding]
buffer = [2, 2]
```

## `spacing`

### `section`

Spacing between sidebar sections.

```toml
# Type: integer
# Values: any non-negative integer
# Default: 2

[sidebar.spacing]
section = 6
```

::: info
The key is also accepted under its IRC-era name, `server`.
:::
