# Getting Started

Frigicom has nothing to connect to by hand: on launch it starts its own private
`logoscore` daemon, loads `chat_module`, and waits for the delivery layer to come
online. The sidebar and status bar show that progress; the composer stays
disabled until the backend reports **online**, which can take anywhere from a few
seconds to a minute on `logos.test`.

::: info
Configuration happens through a `config.toml` file. See
[Configuration](./configuration.md), and [Logos](./configuration/logos.md) for
the backend section specifically.
:::

## Your address

Once online, your address is shown in the account card at the bottom of the
sidebar; click it to copy. This is what other people use to start a conversation
with you.

::: warning
The address is **ephemeral**. `chat_module` mints a new one every time it
initialises and its persistence is compiled off upstream, so your address —
along with every conversation and message — is gone after a restart.
:::

## Starting a conversation

| Command                | Example                    | Description                              |
| ---------------------- | -------------------------- | ---------------------------------------- |
| `/dm <address>`        | `/dm 0a1b2c3d…`            | Open (or create) a direct conversation   |
| `/group <name> [desc]` | `/group pals weekend plan` | Create a group conversation              |
| `/add <address>`       | `/add 0a1b2c3d…`           | Add a member to the focused group        |

The same actions are available from the **New chat** button in the sidebar.

Group membership commits take up to a minute to propagate; added members appear
in the roster as *pending* until they do.

See [Commands](./commands.md) for the full list.
