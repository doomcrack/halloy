# Single Pane

The settings below configure Frigicom to use a single pane (or a fixed number of
panes) in regular use. After applying them, close all but one pane. Activating
another conversation in the sidebar then replaces the view in the sole remaining
pane rather than opening a new one. New panes can still be opened from the
context menu on sidebar items.

```toml
[actions.buffer]
click_username = { open-direct = "replace-pane" }
open_internal = "replace-pane"
message_user = "replace-pane"

[actions.sidebar]
buffer = "replace-pane"
```
