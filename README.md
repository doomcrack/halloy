# frigicom

A standalone desktop chat client for the Logos network. It talks to
`logos-chat-module` through a **private `logoscore` daemon** that it starts and
stops with the app — there are no servers to configure and nothing to log into.

frigicom is a fork of [halloy](https://github.com/squidowl/halloy), an IRC
client by Casper Rogild Storm, Cory Forsstrom, and Andrew Baldwin. It keeps
halloy's chrome — panes, sidebar, themes, keyboard navigation — and replaces the
IRC stack underneath with the Logos backend.

## Status

**Pre-alpha.** Usable end to end against a live daemon, but:

- **Identity is ephemeral upstream.** `chat_module` mints a new address on every
  `init` and its persistence is compiled off, so your address, conversations,
  and messages do not survive a restart.
- No attachments, reactions, replies, receipts, typing indicators, or history
  pagination — none of them exist in the module contract.
- Group membership changes take up to a minute to commit.
- No packaging: build and run from source.

## Quickstart

The Logos artifacts (`liblogos_protocol`, the `logoscore` binary, the staged
modules directory) come from nix:

```sh
nix develop            # or:  . scripts/dev-env.sh
cargo run --features live
```

`live` is **not** a default feature — it links `liblogos_protocol` and needs
`LOGOS_PROTOCOL_ROOT` at build time. For UI work, no daemon, dylib, or nix is
needed: set `mock = true` under `[logos]` in your `config.toml` and

```sh
cargo run
```

drives the UI from a scripted in-process mock instead.

Configuration lives in `config.toml`; see `docs/configuration/logos.md` for the
`[logos]` section and the dev-shell environment variables.

## Architecture

The backend crates live under [`logos/`](logos/README.md) — that README covers
the crate map, the daemon recipe, and the threading rules the FFI imposes
(one dedicated thread per `lp_client`, synchronous invokes only, subscribe
before watching, dead links that report success). `data/` folds the backend's
`Update` stream into domain state; `src/` is the iced UI.

## Licensing and attribution

frigicom is released under **GPL-3.0-or-later**, inherited from halloy. See
[LICENSE](LICENSE).

Based on [halloy](https://github.com/squidowl/halloy) (GPL-3.0-or-later),
copyright its authors, who are retained in `[workspace.package] authors`.
Upstream's theme format is unchanged, so halloy themes and the
`halloy:///theme` deep link still work.

The workspace pins [`squidowl/iced`](https://github.com/squidowl/iced) via
`[patch.crates-io]` — halloy's iced fork, a handful of patches on iced master.
That pin is inherited from upstream and moves when upstream moves it.
