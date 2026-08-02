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
- Packaging is macOS-only and Apple Silicon only, because the Logos artifacts
  are arm64 and there is no x86_64 half to pair them with. Everywhere else
  builds from source.

## Try it

Apple Silicon Mac, macOS 12 or later:

```sh
curl -fsSL https://github.com/doomcrack/halloy/releases/download/v0.0.1-pre-alpha/frigicom.dmg -o /tmp/frigicom.dmg && hdiutil attach /tmp/frigicom.dmg -nobrowse -quiet && cp -R /Volumes/Frigicom/Frigicom.app /Applications/ && hdiutil detach /Volumes/Frigicom -quiet && xattr -dr com.apple.quarantine /Applications/Frigicom.app && open /Applications/Frigicom.app
```

The `xattr` step is part of the line rather than a footnote because it is
not optional: the build is signed ad-hoc, so without it macOS reports the
app as damaged and refuses to open it. It strips the download flag and
changes nothing else.

The release is a pre-release, so the tag is spelled out above —
`releases/latest/` does not resolve to it.

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

## Handing it to someone else

```sh
nix build .#modules        # the module set, recording where it came from
. scripts/dev-env.sh
scripts/package-macos-dmg.sh
```

produces `target/release/frigicom.dmg`: an Apple Silicon `Frigicom.app` that
runs on macOS 12 or later with no nix, no dev shell and no checkout. The
bundle carries its own `logoscore`, its own module set, and about 200 dylib
references the two of them pull in, all rewritten off the nix store by
`scripts/macos-bundle-libs.py` — see `docs/packaging-macos.md` for what that
involves and what is known to break.

**One of the four bundled modules is our fork** — `chat_module` carries a
libchat patch upstream does not have — so the image declares what it
contains in `MODULES.txt`, per module, with the revision each was built
from. Package from a hand-staged module tree instead of `.#modules` and it
says so rather than pretending to know.

The build is signed ad-hoc, so **whoever opens it must run**

```sh
xattr -dr com.apple.quarantine /Applications/Frigicom.app
```

once, or macOS will report the app as damaged. The disk image says so too.

## Architecture

Three layers, and the boundaries between them are licence boundaries too.

| Layer | Where | Licence |
| --- | --- | --- |
| **Modules** — chat, delivery, capability, blockchain | separate upstream repos, loaded by a daemon in their own processes | MIT/Apache-2.0 |
| **Client stack** — daemon supervision, the `lp_*` client, the chat contract, the domain state it folds into | [`doomcrack/logos-rs`](https://github.com/doomcrack/logos-rs), a pinned dependency | MIT/Apache-2.0 |
| **Application** — `src/` (the iced UI), `data/`, `ipc/` | this repository | **GPL-3.0-or-later** |

The middle layer is not derived from halloy and does not depend on this
repository — no GUI framework, no `data`, no `ipc`. That is what let it be
separated, and `tests/layering.rs` fails the build if an edge ever points
back the other way. Its README covers the crate map, the daemon recipe, and
the threading rules the FFI imposes.

`data/` folds the backend's `Update` stream into UI state and re-exports the
domain types under the paths they always had; `src/` is the iced UI.

## Licensing and attribution

frigicom — this repository — is released under **GPL-3.0-or-later**,
inherited from halloy. See [LICENSE](LICENSE). A binary built from it is a
combined work and is GPL too, whatever its dependencies are licensed under.

Based on [halloy](https://github.com/squidowl/halloy) (GPL-3.0-or-later),
copyright its authors, who are retained in `[workspace.package] authors`.
Upstream's theme format is unchanged, so halloy themes and the
`halloy:///theme` deep link still work.

The Logos client stack was written for frigicom but is not derived from
halloy, so it lives in its own repository under MIT/Apache-2.0 — the licence
every Logos component it talks to uses. Separating it is what keeps that
possible; it does not change what this repository or its binaries are under.

The workspace pins [`squidowl/iced`](https://github.com/squidowl/iced) via
`[patch.crates-io]` — halloy's iced fork, a handful of patches on iced master.
That pin is inherited from upstream and moves when upstream moves it.
