# Themes

Theme settings for Frigicom.

## `theme`

::: warning
`theme` is a root key, so it must be placed before every section.
:::

Specify the theme name(s) to use. The theme must correspond to a file in the `themes` folder of your Frigicom [configuration directory](/configuration#directory). The default theme in Frigicom is [Ferra](https://github.com/casperstorm/ferra/).

When multiple themes are specified, Frigicom will randomly select one each time the application starts. When a dynamic theme is used, Frigicom will match the appearance of the OS.

```toml
# Type: string, array of strings, or object
# Values: `"<string>"`, `["<string>", "<string>"]`, `{ light = "<string>", dark = "<string>" }`, `{ light = ["<string>", "<string>"], dark = ["<string>", "<string>"] }`
# Default: `"ferra"`

# Static single
theme = "ferra"
# Static multiple (random selection)
theme = ["ferra", "booberry"]
# Dynamic single
theme = { light = "ferra-light", dark = "ferra" }
# Dynamic multiple (random selection)
theme = { light = ["ferra-light", "booberry-light"], dark = ["ferra", "booberry"] }
```

Discover community created themes for Frigicom at [https://themes.halloy.chat](https://themes.halloy.chat).

What to create your own theme? See [Custom Themes](/guides/custom-themes) guide
