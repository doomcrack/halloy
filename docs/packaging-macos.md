# Packaging frigicom for macOS

What `scripts/package-macos-dmg.sh` does, why each step is there, and the
four things that were measured to be wrong before it worked. Everything
below was established by running the result, not by reading documentation:
the failure modes here are quiet ones, and three of the four produce a
bundle that launches and looks healthy while being broken.

```sh
. scripts/dev-env.sh
scripts/package-macos-dmg.sh          # → target/release/frigicom.dmg
```

Knobs: `FRIGICOM_PROFILE` (default `release`), `FRIGICOM_SIGN_ID` (default
`-`, ad-hoc), `FRIGICOM_SKIP_BUILD`, `FRIGICOM_QT_PLUGINS`,
`FRIGICOM_AUDIT_EXTRA`.

## What has to end up in the bundle

The app is not one binary. It supervises a `logoscore` daemon, which loads
four modules, each in its own `logos_host_qt` process, and every one of
those is a nix build that names its dependencies by absolute
`/nix/store/<hash>-<name>/…` path. Those paths exist on the build machine
and nowhere else — which is the whole reason a copied `.app` produces a
daemon that dies before it logs anything useful.

`daemon/src/locate.rs` already looks for a bundled layout, so the
staging is dictated rather than invented:

```
Frigicom.app/Contents/
  MacOS/frigicom
  Frameworks/                 the relocated dylib closure, ~200 references
  Resources/
    LICENSE                   GPL-3.0-or-later — frigicom itself
    MODULES.txt               generated: what each module is, and whose
    THIRD-PARTY.txt           generated: the bundled dylib closure, named
    licenses/<module>/        MIT + Apache-2.0 texts, staged by the flake
    logos/
      bin/logoscore           a shell wrapper (below)
      libexec/                the real logoscore + logos_host + logos_host_qt
      modules/                chat, delivery, capability, blockchain
      qt-plugins/             tls, networkinformation
```

`locate.rs` searches overrides → environment → bundle → `PATH`, and checks
both `logos/` beside the executable and `../Resources/logos`. A launched
`.app` inherits no shell environment, so on a recipient's machine the
bundle branch is the only one that can fire.

`scripts/macos-bundle-libs.py` does the relocation: it walks every Mach-O
in the bundle, copies each `/nix/store` dependency into `Contents/
Frameworks`, rewrites the load commands to `@rpath`, deletes the store
`LC_RPATH` entries and adds one pointing at `Frameworks` relative to each
file's own location. `build.rs` already emits
`@executable_path/../Frameworks` for the app binary under `--features
live`, so the app half of this was anticipated; the daemon half was not.

## The module set, and why the image declares it

**One of the four modules is ours.** `chat_module` is built from
`doomcrack/logos-chat-module`, which carries a libchat patch for the
duplicate-Welcome abort on top of upstream `0.2.1`. `delivery_module`
(`0.1.3`), `blockchain_module` (tag `0.2.0`) and `capability_module` are
upstream builds.

That is allowed — every one of those sources is dual MIT/Apache-2.0, and
those licences permit redistributing modified binaries provided the terms
travel with them. The problem an undeclared bundle creates is not legal
but epistemic: **a tester who hits a bug cannot tell our patch from
upstream behaviour, and neither can we from their report.** So the image
says so, in `MODULES.txt`, per module, with the flake and revision each
came from.

Which means the tree has to know its own provenance, and a hand-merged one
cannot. `nix build .#modules` produces it instead:

```
result-modules/
  modules/            ← LOGOS_MODULES_DIR
  licenses/<module>/  ← MIT + Apache-2.0, copied from each source
  provenance.json     ← flake + rev + patched? per module, from the lock
```

`scripts/dev-env.sh` prefers that tree over anything in `.gcroots`, and the
packaging script reads `provenance.json` from beside it. Package from a
hand-staged tree instead and the image still builds — but `MODULES.txt`
opens with `PROVENANCE NOT RECORDED` and names only the versions it can
read off the manifests, which is the honest thing for an artifact nobody
can reproduce.

The versions in the record come off the manifests actually staged rather
than being written down a second time, so the record cannot drift from the
tree it describes.

### Licences

- **frigicom** is GPL-3.0-or-later. `LICENSE` ships in `Resources/`, and
  `MODULES.txt` names the source repository — a binary distribution has to
  convey both.
- **The modules** are MIT/Apache-2.0; the flake copies each source's own
  texts into `licenses/<module>/`.
- **Qt** is LGPL-3. It ships as separate dylibs in `Contents/Frameworks`
  rather than statically linked, so the relinking right is intact;
  `THIRD-PARTY.txt` names it alongside the rest of the closure.

## The four traps

### Copying a dylib breaks its own rpaths, before they have been followed

A nix binary reaches its siblings through an `LC_RPATH` written as
`@loader_path/../lib`, and `@loader_path` means *wherever the file is
now*. Copy it into `Frameworks` first and that rpath resolves against
`Frameworks`, so the rest of the closure becomes unreachable at exactly
the moment you need to walk it — silently, because a dependency that
cannot be resolved just is not imported.

The relocator therefore records where each file came from and tries every
relative rpath against **both** locations. Without this,
`libpackage_manager_lib.dylib` is missing from the bundle and the daemon
aborts at load with a dyld error naming eight paths it tried.

### The loader binaries are wrapped twice

`nix` installs each logos binary behind a compiled wrapper whose only job
is to set environment variables to store paths. Unwrapping one level is
the trap, because what is left is *still a wrapper*:

```
logos_host_qt                                   (-qt-bin- derivation)
  └── .logos_host_qt-wrapped                    (-qt-bin-, still a wrapper)
        └── .logos_host_qt-wrapped              (-qt-build-, the real binary)
```

Stage the middle one and the bundle looks complete, launches, loads its
modules — and every module process quietly exec's the build machine's copy
and maps a **second QtCore** alongside the bundle's. `QCoreApplication` is
a static inside QtCore, so the module's copy sees `instance()` as null,
and chat init fails with `QEventLoop: Cannot be used without
QCoreApplication`. The only visible warning is an `objc` duplicate-class
complaint buried in the daemon log.

`makeCWrapper` records its target verbatim in the binary, so the script
walks the chain to the end rather than guessing at its depth.

### `LOGOS_HOST_PATH` must name the *Qt* host

`liblogos_core` prefers `logos_host_qt` and falls back to `logos_host`.
Point `LOGOS_HOST_PATH` at the plain one and it does not use it — it goes
looking for the Qt variant elsewhere, finds the path it was compiled
against, and runs a host out of the store, with the same duplicate-QtCore
result as above. Naming the plain host is not a smaller mistake than
naming none.

### rustc bakes the build machine's home directory into the binary

An unremapped build carries the build user's home directory in every
dependency source path it records — `~/.cargo/registry`, `~/.cargo/git`,
`~/.rustup` — and hands them to whoever opens the image. Measured here:
**4,931** occurrences of the user's name in `target/debug/frigicom`, and
**zero** in the packaged binary. The script exports

```
--remap-path-prefix=$HOME=/build --remap-path-prefix=$PWD=/frigicom
```

(later mappings win, so the checkout rule must follow the home rule) and
then greps the staged bundle for `id -un` and the home directory's name,
refusing to package if either appears. Add more patterns with
`FRIGICOM_AUDIT_EXTRA`. Note this invalidates the build cache: the first
packaged build after a normal one is a full rebuild.

## Qt plugins

Only `tls` and `networkinformation` are staged. They are loaded by name at
runtime rather than linked, so the closure walk cannot discover them: TLS
is how QtNetwork reaches openssl, `networkinformation` is how it notices
the link come and go. Nothing else in the stack is plugin-driven — daemon
and modules between them link QtCore, QtNetwork and QtRemoteObjects and no
more — and staging the rest of Qt's plugin tree drags the whole Qt Quick
stack in behind it: fifty-odd frameworks, none of them ever loaded, and
about 30 MB. `FRIGICOM_QT_PLUGINS` overrides the list.

## Signing

Nested code is signed deepest-first — signing a container seals what is
inside it, so anything signed afterwards invalidates the seal above it —
then the frameworks as bundles, then the app. This has to come last:
`install_name_tool` invalidates a signature, so relocation must be
finished before signing starts.

The default identity is ad-hoc (`-`), which is enough for the app to run
but not enough for Gatekeeper. **Whoever opens the image must run**

```sh
xattr -dr com.apple.quarantine /Applications/Frigicom.app
```

or macOS reports the app as damaged and refuses to open it; on macOS 15
the right-click → Open escape hatch no longer applies. The `README.txt`
inside the image says so. With a Developer ID in `FRIGICOM_SIGN_ID` the
step goes away, but notarization — which `scripts/sign-macos.sh` already
implements for the upstream release path — would still be needed to make
it seamless.

## Limits

- **Apple Silicon only**, not by choice: the logos artifacts are arm64, so
  there is no x86_64 half to `lipo` in.
- **macOS 12 or later**, set by the Qt build the daemon links against
  (`QtCore` reports `minos 12.0`; our own floor is 11.3).
- Chat identity is minted fresh on every `init` upstream, so a recipient's
  conversations do not survive a restart. See the persistence analysis in
  `upstream/`.
- The blockchain module ships in the bundle but is not started — load and
  unload controls are not built yet, and a node that is killed without
  `stop()` loses its chain (`logos-modules.md` §5).
- The disk image is ~115 MB, most of it the module tree.

## Verifying a bundle

The build machine has a nix store, so "it runs here" proves nothing. What
does:

```sh
# 1. nothing in the bundle still names the store
grep -rlF /nix/store target/release/dmg/Frigicom.app | wc -l        # → 0

# 2. run it with no environment at all, against a throwaway HOME
env -i HOME=/tmp/probe PATH=/usr/bin:/bin \
    target/release/dmg/Frigicom.app/Contents/MacOS/frigicom

# 3. while it runs, no logos process may map anything from the store
for p in $(pgrep -f Frigicom.app); do lsof -p $p; done | grep -c /nix/store
```

Step 3 is the one that catches the wrapper trap; steps 1 and 2 both pass
on a bundle that is still running the build machine's module hosts. A good
run reaches `backend phase: Online` and logs `backend ready: address …`
within about five seconds.
