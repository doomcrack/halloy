# Commands

Commands in Frigicom are prefixed with `/`. Anything else typed in the composer
is sent as a plain-text message.

The argument(s) for a command are shown in [tooltips](configuration/tooltips.md).

| Command    | Arguments             | Description                                                                     |
| ---------- | --------------------- | ------------------------------------------------------------------------------- |
| `/dm`      | `[address]`           | Open (or create) a direct conversation. Without an address, opens the New DM dialog. |
| `/group`   | `[name] [description]`| Create a group conversation. Without a name, opens the New Group dialog.        |
| `/add`     | `<address>`           | Add a member to the focused group conversation.                                 |
| `/nick`    | `[nickname]`          | Set a local nickname for the focused conversation. Without one, opens the nickname dialog. |
| `/details` |                       | Toggle the details panel of the focused conversation.                           |
| `/clear`   |                       | Clear the focused buffer's messages.                                            |

::: info
`/nick` is local only — it renames the conversation in your sidebar and does not
change anything for the other participants.
:::

::: warning
`chat_module` carries no message ids, reactions, replies, receipts, or history
pagination, so there are no commands for them. Group membership changes take up
to a minute to commit.
:::
