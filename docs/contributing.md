# Contributing to Frigicom

Frigicom is a fork of [halloy](https://github.com/squidowl/halloy). Bugs in the
chrome that also reproduce on upstream halloy belong upstream; everything about
the Logos backend belongs here.

## Issues and pull requests

Open issues and pull requests on the fork's
[GitHub repository](https://github.com/doomcrack/halloy).

## Building

See [Installation](./installation.md) for the toolchain and the Logos artifacts,
and [`logos/README.md`](https://github.com/doomcrack/halloy/tree/main/logos) for
the backend architecture and the threading rules the `logos/*` crates must obey.

Before submitting:

```sh
cargo check --workspace
cargo clippy -p frigicom --no-deps --all-targets
cargo test -p data -p logos-chat -p frigicom
```

`cargo test -p frigicom` runs the headless UI harness. See [Testing](./testing.md)
for the layers, how to add a case, and how time is controlled.

## Documentation

Any setting added in a pull request should be described in the
[markdown used to generate this site](https://github.com/doomcrack/halloy/tree/main/docs).

## License

Contributions are made under GPL-3.0-or-later, inherited from halloy.
