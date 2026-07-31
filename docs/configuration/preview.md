# Preview

URL preview settings for Frigicom.

## `enabled`

Enable or disable previews globally with a boolean, or selectively enable them for URLs matching specific regex patterns.

```toml
# Type: boolean or array of strings
# Values: true, false, or array of regex patterns
# Default: true

[preview]
enabled = true
```

Only show previews for matching URLs:

::: tip
Use toml multi-line literal strings `'''\bfoo'd\b'''` when writing a regex. This allows you to write the regex without escaping. You can also use a literal string `'\bfoo\b'`, but then you can't use `'` inside the string.

Without literal strings, you'd have to write the above as `"\\bfoo'd\\b"`
:::

```toml
[preview]
enabled = [
    '''https?://(www\.)?imgur\.com/.*''',
    '''https?://(www\.)?dr\.dk/.*'''
]
```

## `exclude`

Exclude URLs from showing previews by providing regex patterns.

```toml
# Type: array of strings
# Values: array of regex patterns
# Default: []

[preview]
exclude = []
```

Prevent previews from showing for matching URLs:

::: tip
Use toml multi-line literal strings `'''\bfoo'd\b'''` when writing a regex. This allows you to write the regex without escaping. You can also use a literal string `'\bfoo\b'`, but then you can't use `'` inside the string.

Without literal strings, you'd have to write the above as `"\\bfoo'd\\b"`
:::

```toml
[preview]
exclude = [
    '''https?://(www\.)?example\.com/.*''',
    '''https?://(www\.)?spam-site\.net/.*'''
]
```

## `max_per_message`

Maximum number of previews to show for a single message.

```toml
# Type: integer
# Values: any non-negative integer
# Default: 1

[preview]
max_per_message = 1
```

## `card`

Specific card preview settings.

### `hide_url`

Hides URL(s) in messages when their card preview is visible and the specified condition is met.

| Condition             | Description                                                                       |
| --------------------- | --------------------------------------------------------------------------------- |
| `"trailing"`          | URL is hidden when it is at the end of a message and its card preview is visible. |
| `"contains-only-url"` | URL is hidden when it is the entire message.                                      |
| `"never"`             | URLs are never hidden.                                                            |

```toml
# Type: string
# Values: "trailing", "contains-only-url", or "never"
# Default: "never"

[preview.card]
hide_url = "contains-only-url"
```

### `show_image`

Show image for card previews.

```toml
# Type: boolean
# Values: true, false
# Default: true

[preview.card]
show_image = true
```

### `round_image_corners`

Round the corners of the image in the card preview (if shown).

```toml
# Type: boolean
# Values: true, false
# Default: true

[preview.card]
round_image_corners = true
```

### `max_width`

Maximum width of the card in pixels.

```toml
# Type: number
# Values: any positive number
# Default: 400.0

[preview.card]
max_width = 400.0
```

### `description_max_height`

Maximum height of the description text in pixels.

```toml
# Type: number
# Values: any positive number
# Default: 100.0

[preview.card]
description_max_height = 100.0
```

### `image_max_height`

Maximum height of the image in the card preview in pixels.

```toml
# Type: number
# Values: any positive number
# Default: 200.0

[preview.card]
image_max_height = 200.0
```

### `image_action`

Action when clicking on the card preview image.

| Action                | Description                                                                       |
| --------------------- | --------------------------------------------------------------------------------- |
| `"open-url"`          | Open the URL in the browser.                                                      |
| `"open-image-url"`    | Open the image URL in the browser.                                                |
| `"preview"`           | Display a larger version of the image in-app.                                     |

```toml
# Type: string
# Values: "open-url", "open-image-url", "preview"
# Default: "open-url"

[preview.card]
image_action = "preview"
```

### `description_decode_html`

`og:description` is always a string, but due to it being in HTML it needs to be
escaped. Sometimes you may get double-escaped entries in this field, this
setting will allow you to handle this scenario.

::: warning
Using this option may break purposely created `og:description` entries meant to include
HTML escape codes.
:::

```toml
# Type: boolean or array of strings
# Values: true, false, or array of regex patterns
# Default: false

[preview.card]
description_decode_html = true
```

To whitelist specific domains, you can use an array of regex patterns.

::: tip
Use toml multi-line literal strings `'''\bfoo'd\b'''` when writing a regex. This allows you to write the regex without escaping. You can also use a literal string `'\bfoo\b'`, but then you can't use `'` inside the string.

Without literal strings, you'd have to write the above as `"\\bfoo'd\\b"`
:::

```toml
[preview.card]
description_decode_html = [
    '''https?://(www\.)?github\.com/.*''',
]
```

## `image`

Specific image preview settings.

### `action`

Action when clicking on a image. `open-url` will open the image in the browser, and `preview` will display a larger version of the image in-app.

```toml
# Type: string
# Values: "open-url", "preview"
# Default: "preview"

[preview.image]
action = "preview"
```

### `hide_url`

Hides URL(s) in messages when their image preview is visible and the specified condition is met.

| Condition             | Description                                                                        |
| --------------------- | ---------------------------------------------------------------------------------- |
| `"trailing"`          | URL is hidden when it is at the end of a message and its image preview is visible. |
| `"contains-only-url"` | URL is hidden when it is the entire message.                                       |
| `"never"`             | URLs are never hidden.                                                             |

```toml
# Type: string
# Values: "trailing", "contains-only-url", or "never"
# Default: "contains-only-url"

[preview.image]
hide_url = "trailing"
```

### `round_corners`

Round the corners of the image.

```toml
# Type: boolean
# Values: true, false
# Default: true

[preview.image]
round_corners = true
```

### `max_width`

Maximum width of the image in pixels.

```toml
# Type: number
# Values: any positive number
# Default: 550.0

[preview.image]
max_width = 550.0
```

### `max_height`

Maximum height of the image in pixels.

```toml
# Type: number
# Values: any positive number
# Default: 350.0

[preview.image]
max_height = 350.0
```

## `image_cache`

Settings to control how the image cache is managed. The cache is stored in:

- Windows: `%AppData%\Roaming\Local\frigicom\previews\images\`
- Mac: `~/Library/Caches/frigicom/previews/images/`
- Linux: `$XDG_CACHE_HOME/frigicom/previews/images/` or `$HOME/.cache/frigicom/previews/images/`

### `max_size`

Maximum size in MB for cached preview images, or `"unlimited"` for an uncapped image cache (not recommended).

```toml
# Type: integer
# Values: any non-negative integer or "unlimited"
# Default: 500

[preview.request.image_cache]
max_size = 500
```

### `trim_interval`

Run image cache trimming every N successful image saves. Set to `"first-save-only"` to disable periodic trimming, and only trim on the first save to the image cache per app session.

```toml
# Type: integer
# Values: any non-negative integer or "first-save-only"
# Default: 32

[preview.request.image_cache]
trim_interval = 32
```

## `request`

Request settings for previews.

### `user_agent`

Some servers will only send opengraph metadata to browser-like user agents. We default to `WhatsApp/2` for wide compatibility.

```toml
# Type: string
# Values: any string
# Default: "WhatsApp/2"

[preview.request]
user_agent = "WhatsApp/2"
```

### `timeout_ms`

Request timeout in milliseconds. Defaults is 10s.

```toml
# Type: integer
# Values: any non-negative integer
# Default: 10000

[preview.request]
timeout_ms = 10000
```

### `max_image_size`

Max image size in bytes. This prevents downloading responses that are too big. Default is 10mb.

```toml
# Type: integer
# Values: any non-negative integer
# Default: 10485760

[preview.request]
max_image_size = 10485760
```

### `max_scrape_size`

Max bytes streamed when scraping for opengraph metadata before cancelling the request. This prevents downloading responses that are too big. Default is 500kb.

```toml
# Type: integer
# Values: any non-negative integer
# Default: 512000

[preview.request]
max_scrape_size = 512000
```

### `concurrency`

Number of allowed concurrent requests for fetching previews. Reduce this to prevent rate-limiting.

```toml
# Type: integer
# Values: any non-negative integer
# Default: 4

[preview.request]
concurrency = 4
```

### `delay_ms`

Number of milliseconds to wait before requesting another preview when number of requested previews > `concurrency`.

```toml
# Type: integer
# Values: any non-negative integer
# Default: 500

[preview.request]
delay_ms = 500
```
