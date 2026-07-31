<!-- markdownlint-disable MD033 -->

# Notifications

Customize and enable notifications.

```toml
[notifications]
direct_message = { sound = "peck", show_toast = true }

[notifications.group_invite]
sound = "dong"
show_toast = true
```

## Types

The following notifications are available:

| Name             | Description                                              | Content      |
| ---------------- | -------------------------------------------------------- | ------------ |
| `direct_message` | Triggered when a message is received in a direct conversation | Message text |
| `group_message`  | Triggered when a message is received in a group conversation  | Message text |
| `group_invite`   | Triggered when you are added to a group conversation      | Group name   |
| `connected`      | Triggered when the backend first comes online            | N/A          |
| `disconnected`   | Triggered when the backend leaves the online state       | N/A          |
| `reconnected`    | Triggered when the backend comes back online             | N/A          |

## Built-in Sounds

The following table shows all available built-in sounds

| Sound Name | Preview                                                                       |
| ---------- | ----------------------------------------------------------------------------- |
| `bloop`    | <audio controls><source src="../sounds/bloop.ogg" type="audio/ogg"></audio>   |
| `bonk`     | <audio controls><source src="../sounds/bonk.ogg" type="audio/ogg"></audio>    |
| `dong`     | <audio controls><source src="../sounds/dong.ogg" type="audio/ogg"></audio>    |
| `drop`     | <audio controls><source src="../sounds/drop.ogg" type="audio/ogg"></audio>    |
| `peck`     | <audio controls><source src="../sounds/peck.ogg" type="audio/ogg"></audio>    |
| `ring`     | <audio controls><source src="../sounds/ring.ogg" type="audio/ogg"></audio>    |
| `sing`     | <audio controls><source src="../sounds/sing.ogg" type="audio/ogg"></audio>    |
| `squeak`   | <audio controls><source src="../sounds/squeak.ogg" type="audio/ogg"></audio>  |
| `tweep`    | <audio controls><source src="../sounds/tweep.ogg" type="audio/ogg"></audio>   |
| `whistle`  | <audio controls><source src="../sounds/whistle.ogg" type="audio/ogg"></audio> |
| `zone`     | <audio controls><source src="../sounds/zone.ogg" type="audio/ogg"></audio>    |

## `sound`

Notification sound. Supports both built-in sounds and external sound files
(`.ogg`, `.mp3`, `.flac`, `.wav`) placed in a `sounds` folder inside the
[configuration directory](/configuration#directory).

```toml
# Type: string
# Values: see above for built-in sounds, eg: "zone" or external sound.
# Default: not set

[notifications.direct_message]
sound = "peck"
```

## `show_toast`

Notification should trigger a OS toast.

```toml
# Type: boolean
# Values: true, false
# Default: false

[notifications.direct_message]
show_toast = true
```

## `request_attention`

Request attention from the OS (bounce the dock icon on macOS, flash the
taskbar entry on Windows and Linux) when the window is not focused.

```toml
# Type: boolean
# Values: true, false
# Default: false

[notifications.direct_message]
request_attention = true
```

## `show_content`

Include the message content in the OS toast.

```toml
# Type: boolean
# Values: true, false
# Default: false

[notifications.direct_message]
show_content = true
```

## `delay`

Minimum delay in milliseconds between two notifications of this type.

```toml
# Type: integer
# Values: any non-negative integer
# Default: 500

[notifications.direct_message]
delay = 250
```
