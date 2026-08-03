# LGX variant token for Intel macOS: packer writes `darwin-amd64`, `lgpm` wants `darwin-x86_64`

- **Where:** the shared LGX-bundler flake (`nix-bundle-lgx` / `logos-package`),
  `flake.nix:26`; surfaces through every Logos module flake that stages a darwin
  `.lgx` (`logos-logoscore-cli`, `logos-chat-module`, `keystore-signer-module`,
  `logos-delivery-module`).
- **Platform:** `x86_64-darwin` only. `aarch64-darwin` and all Linux targets are
  unaffected.
- **Severity:** blocks `nix build .#modules` and the `logoscore` runtime bundle
  on Intel macOS outright — the whole module tree fails to build.
- **Measured on:** frigicom `b00501fc`, macOS 15.7.7 Intel, nix 2.34.7, Attic
  cache enabled.

## Summary

On `x86_64-darwin` the LGX packer stamps module packages with the variant key
**`darwin-amd64-dev`**, but `lgpm` resolves the host platform as
**`darwin-x86_64-dev`** at install (and load) time. The names never match, so
`lgpm install` rejects a package that in fact contains a correct x86_64 payload.
On arm64 both sides say `arm64`, so the bug is invisible there.

## Reproduction

```
git clone -b frigicom https://github.com/doomcrack/halloy.git
cd halloy
nix build .#modules --accept-flake-config -L
```

Fails at the first module install:

```
logos-capability_module-module-lib-lgx> Adding variant darwin-amd64-dev to
    logos-capability_module-module-lib.lgx (main: capability_module_plugin.dylib)
logos-capability_module-module-lib-install> Error: Package does not contain
    variant for platform: darwin-x86_64-dev
error: Cannot build '…-logos-capability_module-module-lib-install.drv'.
      → cascades to logos-logoscore-cli-{modules,bin,cli} → frigicom-modules
```

The produced `.lgx` is internally consistent and carries a genuine Intel binary:

- `manifest.json` `main` = `{"darwin-amd64-dev": "capability_module_plugin.dylib"}`
- `file …/capability_module_plugin.dylib` → `Mach-O 64-bit … shared library x86_64`

so this is a naming/packaging defect, **not** a compile or port gap.

## Root cause

`flake.nix:26` of the bundler:

```nix
(if pkgs.stdenv.isAarch64 then "darwin-arm64" else "darwin-amd64")
```

`bundle.sh` forwards `$VARIANT` verbatim to `lgpm … --variant`. `lgpm`'s own host
resolver reports Intel macOS as `x86_64` (Qt `QSysInfo` CPU-architecture string),
giving `darwin-x86_64-dev`. The darwin branch uses the Linux/Go `amd64` token
where the loader uses the Apple/`uname` `x86_64` token. Linux is self-consistent
(`linux-amd64` on both sides); only darwin-Intel diverges.

No fixed rev exists upstream: `frigicom`'s `flake.lock` pins `nix-bundle-lgx`
**181 times across three `logos-co` revs** (`3c44d99b`, `9d8f8602`, `b49074a8`),
and line 26 reads `darwin-amd64` in all three — and at `logos-co` HEAD
(`b49074a8`). The change has to be made, then propagated through the module
flakes' pins (`nix-bundle-lgx` is transitive under the `logos-module` builder, so
there is no clean single-flag local override).

## Suggested fix

Preferred — align the packer with the loader on darwin:

```diff
- (if pkgs.stdenv.isAarch64 then "darwin-arm64" else "darwin-amd64")
+ (if pkgs.stdenv.isAarch64 then "darwin-arm64" else "darwin-x86_64")
```

and bump the `nix-bundle-lgx` / `logos-package` pin to that rev in every Logos
module flake listed above. Equivalent alternative: make `lgpm`'s darwin host
resolver treat `amd64` and `x86_64` as aliases. Bumping the pin is cleaner since
only darwin-Intel is inconsistent.

## Impact if left unfixed

- No Logos module (`capability`, `chat`, `delivery`, `keystore`) can be installed
  or loaded on Intel macOS via the current pins.
- `logoscore`'s runtime bundle never assembles (the same `lgpm install` is on its
  build path), so there is no runnable daemon on Intel either — the standalone
  binary fails with `dyld: Library not loaded: @rpath/liblogos_core.dylib`.
- Consequence: frigicom's **live** backend cannot start on Intel; only `mock`
  mode runs there today.
