# Buffer

Buffer settings for Frigicom.

## `line_spacing`

Setting to control spacing between messages in buffers

```toml
# Type: integer
# Values: positive integers
# Default: 0

[buffer]
line_spacing = 4
```

## `backlog_separator`

Customize when the backlog separator is displayed within a buffer

### `hide_when_all_read`

Hide backlog divider when all messages in the buffer have been marked as read.

```toml
# Type: boolean
# Values: true, false
# Default: true

[buffer.backlog_separator]
hide_when_all_read = true
```

### `text`

Set the text for backlog divider or disable it

```toml
# Type: boolean or string
# Values: boolean or any string
# Default: true

[buffer.backlog_separator]
text = false
```

## `conversation`

Defaults for conversation buffers. Individual buffers can override these from
their context menu.

### `member_list`

The roster column shown alongside group conversations.

#### `enabled`

```toml
# Type: boolean
# Values: true, false
# Default: true

[buffer.conversation.member_list]
enabled = true
```

#### `position`

```toml
# Type: string
# Values: "left", "right"
# Default: "right"

[buffer.conversation.member_list]
position = "left"
```

#### `width`

```toml
# Type: number
# Values: any positive number
# Default: not set

[buffer.conversation.member_list]
width = 180.0
```

#### `alignment`

```toml
# Type: string
# Values: "left", "right"
# Default: "left"

[buffer.conversation.member_list]
alignment = "right"
```

#### `truncate`

Truncate member names longer than the given number of characters.

```toml
# Type: integer
# Values: any non-negative integer
# Default: not set

[buffer.conversation.member_list]
truncate = 12
```

### `description_banner`

The group description shown under the buffer header.

#### `enabled`

```toml
# Type: boolean
# Values: true, false
# Default: true

[buffer.conversation.description_banner]
enabled = true
```

#### `max_lines`

```toml
# Type: integer
# Values: any non-negative integer
# Default: 2

[buffer.conversation.description_banner]
max_lines = 3
```

::: info
The section is also accepted under its IRC-era names: `[buffer.channel]` with
`nicklist` and `topic_banner`.
:::

## `date_separators`

Customize how date separators are displayed within a buffer

### `format`

Controls the date format. The expected format is [strftime](https://pubs.opengroup.org/onlinepubs/007908799/xsh/strftime.html).

```toml
# Type: string
# Values: any valid strftime string
# Default: "%A, %B %-d"

[buffer.date_separators]
format = "%A, %B %-d"
```

### `show`

Show date separators.

```toml
# Type: boolean
# Values: true, false
# Default: true

[buffer.date_separators]
show = true
```

## `emojis`

Emojis settings.

```toml
[buffer.emojis]
show_picker = true
skin_tone = "default"
auto_replace = true
```

### `show_picker`

Show the emoji picker when typing `:shortcode:` in text input.

```toml
# Type: boolean
# Values: true, false
# Default: true

[buffer.emojis]
show_picker = true
```

### `skin_tone`

Skin tone selected when picking an emoji.

```toml
# Type: string
# Values: "default", "light", "medium-light", "medium", "medium-dark", "dark"
# Default: "default"

[buffer.emojis]
skin_tone = "default"
```

### `auto_replace`

Automatically replace `:shortcode:` in text input with the corresponding emoji.

```toml
# Type: boolean
# Values: true, false
# Default: true

[buffer.emojis]
auto_replace = true
```

### `characters_to_trigger_picker`

Minimum number of characters after `:` required for the emoji picker to show.
E.g. `:D` will not show the emoji picker unless `characters_to_trigger_picker` is less than or equal to `1`.

```toml
# Type: integer
# Values: any non-negative integer
# Default: 2

[buffer.emojis]
characters_to_trigger_picker = 2
```

## `mark_as_read`

When to mark a buffer as read

### `on_application_exit`

When exiting the application (all buffers, opened or closed, will be marked as read when the application exits).

```toml
# Type: boolean
# Values: true, false
# Default: false

[buffer.mark_as_read]
on_application_exit = false
```

### `on_buffer_close`

When closing a buffer (a buffer is considered closed when it is replaced or if it is open when the application exits).  If set to `"scrolled-to-bottom"` then a buffer will only be marked as read if it is scrolled to the bottom when closing (i.e. if the most recent messages are visible).

```toml
# Type: boolean
# Values: true, false, "scrolled-to-bottom"
# Default: "scrolled-to-bottom"

[buffer.mark_as_read]
on_buffer_close = "scrolled-to-bottom"
```

### `on_scroll_to_bottom`

When scrolling to the bottom of a buffer.

```toml
# Type: boolean
# Values: true, false
# Default: true

[buffer.mark_as_read]
on_scroll_to_bottom = true
```

### `on_message_sent`

When sending a message to the buffer.

```toml
# Type: boolean
# Values: true, false
# Default: true

[buffer.mark_as_read]
on_message_sent = true
```

### `on_message`

Marks as read when Frigicom is focused and a new message arrives in a buffer that is scrolled to the bottom.  If `"focused"` then only the currently focused buffer will have new messages marked as read, while `"open"` will mark messages as read for any open buffer.

```toml
# Type: boolean
# Values: "focused", "open", "none"
# Default: "focused"

[buffer.mark_as_read]
on_message = "open"
```

## `close`

Side effects for when closing buffers.

### `direct`

What happens when closing a direct conversation's buffer. `"keep"` only closes
the pane, while `"close"` also removes the conversation from the sidebar.

```toml
# Type: string
# Values: "close", "keep"
# Default: "keep"

[buffer.close]
direct = "keep"
```

::: info
The key is also accepted under its IRC-era name, `query`.
:::

## `sender`

Styling for the message sender label.

### `alignment`

```toml
# Type: string
# Values: "left", "right", "top"
# Default: "left"

[buffer.sender]
alignment = "top"
```

### `brackets`

```toml
# Type: object
# Values: { left = "<any string>", right = "<any string>" }
# Default: { left = "", right = "" }

[buffer.sender]
brackets = { left = "<", right = ">" }
```

### `color`

`"unique"` derives a stable color per address, matching the avatar ramp.

```toml
# Type: string or object
# Values: "solid", "unique", or { palette = ["#RRGGBB", ...] }
# Default: "unique"

[buffer.sender]
color = "solid"
```

### `truncate`

```toml
# Type: integer
# Values: any non-negative integer
# Default: not set

[buffer.sender]
truncate = 12
```

### `hide_consecutive`

Hide the sender label when the previous message came from the same sender.
`{ smart = <seconds> }` only hides it when the two messages are within that many
seconds of each other.

#### `enabled`

```toml
# Type: boolean or object
# Values: true, false, or { smart = integer }
# Default: false

[buffer.sender.hide_consecutive]
enabled = { smart = 120 }
```

#### `show_after_previews`

```toml
# Type: boolean
# Values: true, false
# Default: false

[buffer.sender.hide_consecutive]
show_after_previews = true
```

::: info
The section is also accepted under its IRC-era name, `[buffer.nickname]`.
:::

## `text_input`

Customize the text input for in buffers.

### `visibility`

Text input visibility. When set to `"focused"` it will only be visible when the buffer is focused.

```toml
# Type: string
# Values: "always", "focused"
# Default: "always"

[buffer.text_input]
visibility = "always"
```

### `auto_format`

Control if the text input should auto format the input. By default text is only formatted when using the `/format` command.

```toml
# Type: string
# Values: "disabled", "markdown", "all"
# Default: "disabled"

[buffer.text_input]
auto_format = "markdown"
```

::: warning
`chat_module` carries plain text only, so formatting is a local rendering
convenience — it is not transmitted.
:::

### `key_bindings`

Different key bindings for the text input

```toml
# Type: string
# Values: "default", "emacs"
# Default: "emacs" on macOS, "default" for all other OSes

[buffer.text_input]
key_bindings = "emacs"
```

##### `emacs`

Emacs variant has the following binds:

> `ctrl+a`: Move the cursor to the beginning of the line
  `ctrl+e`: Move the cursor to the end of the line
  `ctrl+b`: Move the cursor backward one character
  `ctrl+f`: Move the cursor forward one character
  `ctrl+d`: Delete the character under the cursor
  `ctrl+k`: Delete from the cursor to the end of the line
  `ctrl+u`: Delete from the cursor to the beginning of the line
  `ctrl+w`: Delete to the beginning of the word under the cursor
  `alt+b`: Move the cursor backward one word
  `alt+f`: Move the cursor forward one word

::: info
Global [keyboard shortcuts](/configuration/keyboard) take precedence. Unset any that collide (e.g., set `command_bar = "unset"`).
:::

### `kill_to_clipboard`

If enabled, certain key bindings move killed (deleted) text to the clipboard.

```toml
# Type: boolean
# Values: true, false
# Default: true

[buffer.text_input]
kill_to_clipboard = true
```

### `max_lines`

Maximum number of lines in a single input. Longer input is sent as separate messages, [`send_line_delay`](#send_line_delay) milliseconds apart.

```toml
# Type: integer
# Values: > 0
# Default: 5

[buffer.text_input]
max_lines = 5
```

### `send_line_delay`

Delay (milliseconds) between each line when sending multiple lines. `chat_module` is single-dispatch, so a slow send can delay the following lines further.

```toml
# Type: integer
# Values: >= 0
# Default: 100

[buffer.text_input]
send_line_delay = 100
```

### `persist`

If enabled, saves unsent messages on disk.

```toml
# Type: boolean
# Values: true, false
# Default: true

[buffer.text_input]
persist = true
```

### `autocomplete`

Customize autocomplete.

#### `order_by`

Ordering that autocomplete uses to select from matching members.

- `"recent"`: Autocomplete members by their last message in the conversation; the member with the most recent message autocompletes first, then increasingly older messages. Members with no seen messages are matched last, in the order specified by `sort_direction`.
- `"alpha"`: Autocomplete members based on alphabetical ordering of potential matches. Ordering is ascending/descending based on `sort_direction`.

```toml
# Type: string
# Values: "alpha", "recent"
# Default: "recent"

[buffer.text_input.autocomplete]
order_by = "recent"
```

#### `sort_direction`

Sort direction when autocompleting alphabetically.

- `"asc"`: ascending alphabetical (a→z)
- `"desc"`: descending alphabetical (z→a)

```toml
# Type: string
# Values: "asc", "desc"
# Default: "asc"

[buffer.text_input.autocomplete]
sort_direction = "asc"
```

#### `completion_suffixes`

Sets what suffix is added after autocompleting. The first option is for when a name is autocompleted at the beginning of a sentence. The second is for when it's autocompleted in the middle of a sentence.

```toml
# Type: array of 2 strings
# Values: array of 2 strings
# Default: [": ", " "]

[buffer.text_input.autocomplete]
completion_suffixes = [": ", " "]
```

## `timestamp`

Customize how timestamps are displayed within a buffer.

### `format`

Controls the timestamp format. The expected format is [strftime](https://pubs.opengroup.org/onlinepubs/007908799/xsh/strftime.html).

```toml
# Type: string
# Values: any valid strftime string
# Default: "%R"

[buffer.timestamp]
format = "%R"
```

### `context_menu_format`

Controls the format of shown in a timestamp's context menu. The expected format is [strftime](https://pubs.opengroup.org/onlinepubs/007908799/xsh/strftime.html).

```toml
# Type: string
# Values: any valid strftime string
# Default: "%x"

[buffer.timestamp]
context_menu_format = "%x"
```

### `copy_format`

Controls the format used when copying the timestamp into the clipboard from its context menu. The expected format is [strftime](https://pubs.opengroup.org/onlinepubs/007908799/xsh/strftime.html).  If not set, then the timestamp is copied as [ISO 8601:2004(E) 4.3.2 UTC with millisecond precision](https://en.wikipedia.org/wiki/ISO_8601).

```toml
# Type: string
# Values: any valid strftime string or not set
# Default: not set

[buffer.timestamp]
copy_format = "%Y-%m-%d %H:%M:%S"
```

### `brackets`

Brackets around timestamps.

```toml
# Type: string
# Values: { left = "<any string>", right = "<any string>" }
# Default: { left = "", right = "" }

[buffer.timestamp]
brackets = { left = "[", right = "]" }
```

### `locale`

Locale used when formatting timestamps, for strftime formats that produce locale-specific output (e.g. `%x`, `%X`, `%a`, etc).  If not specified, then the locale will be set automatically, falling back to the POSIX locale if the system locale cannot be determined.  Supported locales are determined by [`enum Locale` in the `pure-rust-locales` crate](https://docs.rs/pure-rust-locales/latest/pure_rust_locales/enum.Locale.html).

```toml
# Type: string
# Values: IETF BCP 47 language tags
# Default: not set

[buffer.timestamp]
locale = "POSIX"
```

### `hide_consecutive`

Hide timestamp for consecutive messages from the same user.

If specified as `{ smart = integer }` then the timestamp is hidden only when
the previous message is from the same user and sent within `smart` seconds.

```toml
# Type: boolean
# Values: true, false, or { smart = integer }
# Default: false

[buffer.timestamp.hide_consecutive]
enabled = true

# hide if the previous message was from the same user and sent within 2m of the current message
[buffer.timestamp.hide_consecutive]
enabled = { smart = 120 }
```

## `url`

Customize how urls behave in buffers

### `prompt_before_open`

Prompt before opening a hyperlink.

```toml
# Type: boolean
# Values: true, false
# Default: false

[buffer.url]
prompt_before_open = true
```
