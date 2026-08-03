# delivery module fails to link from source on x86_64-darwin (missing libc++)

- **Where:** `logos-delivery-module` (`liblogosdelivery` — Nim/waku + vendored
  BoringSSL), from-source build on `x86_64-darwin`.
- **Platform:** `x86_64-darwin`. Not seen on arm64-darwin or Linux (those have a
  working prebuilt in the binary cache, or link correctly).
- **Severity:** blocks any frigicom Intel build that re-derives the delivery
  `.lgx` — including the `nix-bundle-lgx` variant fix (issue #0001), because
  changing that input re-derives delivery.

## Symptom

Building `liblogosdelivery-dev` from source on Intel macOS fails at link:

```
liblogosdelivery-dev> Undefined symbols for architecture x86_64:
  "std::logic_error::logic_error(char const*)", referenced from ... (bssl objects)
  "std::out_of_range::~out_of_range()"
  "std::terminate()"
  "operator delete(void*, unsigned long)"
  "typeinfo for std::out_of_range", "vtable for std::out_of_range", ...
```

All undefined symbols are C++ standard-library / libc++ symbols, referenced from
the vendored BoringSSL (`bssl::…`) objects. This is the classic "C++ objects
linked without the C++ standard library" failure — the final link is done with a
driver/flags that don't pull in `libc++` (e.g. `clang` instead of `clang++`, or a
missing `-lc++`).

## Why it matters here

The delivery module's prebuilt **x86_64 dylib substitutes fine from the cache** —
only the *from-source* build is broken. But the `nix-bundle-lgx` fix (issue #0001)
changes an input in delivery's subtree, so nix re-derives the delivery `.lgx`,
which forces this from-source build, which fails. Net effect: even with #0001
applied, `nix build .#modules` cannot complete on Intel until delivery either
(a) links correctly from source, or (b) is consumed strictly as a cached prebuilt
with its variant relabeled out-of-band.

## Suggested fix

Make the delivery link step use the C++ driver / link libc++ on darwin — e.g. in
the Nim build config for the module, ensure the final link uses `clang++` (or add
`--passL:-lc++` / `-lc++abi`) on `x86_64-darwin`. Likely a one-line change in the
delivery module's `nim.cfg`/nix `buildPhase`. (Unverified — needs a maintainer
with the delivery build to confirm the exact hook.)

## Reproduction

```
cd ~/frigicom
# force delivery to re-derive (any change to nix-bundle-lgx in its subtree does this)
scripts/nbl.sh fork build .#modules   # → liblogosdelivery-dev link failure
```

Discovered while consuming the issue-#0001 fix on Intel; see docs/logos-modules.md §8.
