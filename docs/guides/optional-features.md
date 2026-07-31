# Optional Features

Frigicom has a small number of compile-time cargo features.

```bash
cargo build --release --features live
```

## `live`

Links `liblogos_protocol` and enables the FFI transport, i.e. the real backend.
**Not enabled by default**: it needs `LOGOS_PROTOCOL_ROOT` at build time, which
CI runners and packaging scripts do not have.

Build it from the dev shell (`nix develop`, or `. scripts/dev-env.sh`). Without
it, the binary can only drive the scripted mock and `[logos] mock = false` fails
with `FfiUnavailable`. See [Logos](../configuration/logos.md).

## `iosevka-font`

Bundles the default font, Iosevka Term. Enabled by default.
