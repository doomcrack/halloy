# Installing Frigicom

There are no pre-built binaries or packages yet — build from source.

## Requirements

* [Rust toolchain](https://www.rust-lang.org/tools/install) (stable; the
  workspace tracks recent stable, so `rustup update` first)
* [Git](https://git-scm.com/)
* Platform packages:
  - Fedora-based distributions: `alsa-lib-devel openssl-devel libxcb-devel`
  - Debian-based distributions: `librust-alsa-sys-dev libssl-dev libxcb1-dev`
* The Logos artifacts — `liblogos_protocol`, the `logoscore` binary, and the
  staged module directory. Both [Nix](https://nixos.org/download/) dev shells
  in the repository provide them; see below.

## Build

```sh
git clone https://github.com/doomcrack/halloy.git
cd halloy

# Logos artifacts on the environment (either one):
nix develop
# ... or, without nix:
. scripts/dev-env.sh

cargo build --release --features live
cargo run --release --features live
```

The `live` feature links `liblogos_protocol` and is **not** enabled by default,
because it needs `LOGOS_PROTOCOL_ROOT` at build time. Without it the binary can
only drive the scripted mock — see [`[logos] mock`](./configuration/logos.md#mock).

## UI-only build

For UI work no daemon, dylib, or nix is required:

```sh
cargo run
```

with `mock = true` under `[logos]` in your `config.toml`.
