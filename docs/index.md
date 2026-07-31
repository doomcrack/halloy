# Frigicom

A standalone desktop chat client for the Logos network, built with Rust and
[iced](https://github.com/iced-rs/iced/).

::: warning
Pre-alpha. Identity is ephemeral upstream — `chat_module` mints a new address
on every start and its persistence is compiled off, so conversations and
messages do not survive a restart.
:::

## What it is

Frigicom talks to `logos-chat-module` through a **private `logoscore` daemon**
that it starts and stops with the app. There are no servers to configure and
nothing to log into: the [`[logos]` section](./configuration/logos.md) points
the app at the daemon and the module artifacts, and the app does the rest.

It is a fork of [halloy](https://github.com/squidowl/halloy), an IRC client, and
keeps its chrome — panes, sidebar, themes, keyboard navigation. The IRC stack
underneath has been replaced.

## Getting started

Frigicom is not packaged yet. See [Installation](./installation.md) for building
from source, then [Getting Started](./getting-started.md).

## Customization

Frigicom is configured through a `config.toml` file. From the backend and
notifications to themes, fonts, panes, and keyboard shortcuts, there are many
options available. See [Configuration](./configuration.md) to get started.

## License

Frigicom is released under GPL-3.0-or-later, inherited from halloy.
