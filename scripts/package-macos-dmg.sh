#! /usr/bin/env bash

# Builds a Frigicom.app that runs on a Mac with no nix, no dev shell and no
# checkout, and wraps it in a disk image you can hand to someone.
#
#   . scripts/dev-env.sh
#   scripts/package-macos-dmg.sh
#
# The bundle carries its own logoscore, its own module set, and the whole
# dylib closure the two of them pull in. What makes that necessary is that
# every logos artifact is nix-built, so it names its dependencies by
# absolute store path; `macos-bundle-libs.py` is what rewrites those.
#
# Apple Silicon only, and not by choice — the logos artifacts are arm64,
# so there is no x86_64 half to lipo in. macOS 12 is the floor, set by the
# Qt build the daemon links against.
#
# Knobs, all optional:
#   FRIGICOM_PROFILE      cargo profile (default: release; `packaging`
#                         adds fat LTO and roughly triples the build)
#   FRIGICOM_SIGN_ID      codesign identity (default: `-`, ad-hoc)
#   FRIGICOM_AUDIT_EXTRA  extra strings the bundle must not contain
#   FRIGICOM_SKIP_BUILD   reuse the binary already in target/

set -euo pipefail

cd "$(dirname "$0")/.."

PROFILE="${FRIGICOM_PROFILE:-release}"
SIGN_ID="${FRIGICOM_SIGN_ID:--}"
APP_NAME="Frigicom.app"
STAGE="target/$PROFILE/dmg"
APP="$STAGE/$APP_NAME"
LOGOS="$APP/Contents/Resources/logos"
DMG="target/$PROFILE/frigicom.dmg"

step() { printf '\n==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
real() { python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$1"; }

# ---------------------------------------------------------------- preflight

[ "$(uname -s)" = "Darwin" ] || die "macOS only"
[ "$(uname -m)" = "arm64" ] || die "the logos artifacts are arm64; this is $(uname -m)"

for tool in cargo install_name_tool codesign hdiutil python3; do
    command -v "$tool" >/dev/null || die "$tool not found"
done

if [ -z "${LOGOS_MODULES_DIR:-}" ] || [ -z "${LOGOSCORE_BIN:-}" ]; then
    step "sourcing scripts/dev-env.sh for the artifact paths"
    # shellcheck source=/dev/null
    . scripts/dev-env.sh
fi

[ -x "${LOGOSCORE_BIN:-}" ] || die "LOGOSCORE_BIN is not an executable: ${LOGOSCORE_BIN:-<unset>}"
[ -d "${LOGOS_MODULES_DIR:-}" ] || die "LOGOS_MODULES_DIR is not a directory: ${LOGOS_MODULES_DIR:-<unset>}"

for module in chat_module delivery_module capability_module; do
    [ -d "$LOGOS_MODULES_DIR/$module" ] || \
        die "$module missing from $LOGOS_MODULES_DIR — the app will not come up"
done

# -------------------------------------------------------------------- build

if [ -z "${FRIGICOM_SKIP_BUILD:-}" ]; then
    step "building frigicom --features live (profile: $PROFILE)"

    # Without remapping, rustc bakes the absolute path of every dependency
    # source file into the binary — thousands of them, each carrying the
    # build user's name straight into the disk image. Later mappings win,
    # so the checkout rule has to follow the home rule.
    export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=/build --remap-path-prefix=$PWD=/frigicom"
    export MACOSX_DEPLOYMENT_TARGET="12.0"

    cargo build --profile "$PROFILE" --features live --locked
fi

BINARY="target/$PROFILE/frigicom"
[ -f "$BINARY" ] || die "no binary at $BINARY"

# -------------------------------------------------------------------- stage

step "staging $APP_NAME"

# A previous stage is full of read-only directories copied out of the
# store, and `rm` cannot unlink out of a directory it cannot write.
chmod -R u+w "$STAGE" 2>/dev/null || true
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp -R "assets/macos/Halloy.app" "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" \
    "$LOGOS/bin" "$LOGOS/libexec" "$LOGOS/modules" "$LOGOS/qt-plugins"

VERSION=$(grep -q '\..*\.' VERSION && cat VERSION || echo "$(cat VERSION).0")
BUILD=$(git describe --always --dirty --exclude='*' 2>/dev/null || echo unknown)

# Substituted on the copy: build-macos.sh edits the template in place,
# which leaves the checkout dirty and the placeholders spent.
sed -i '' -e "s/{{ VERSION }}/$VERSION/g" -e "s/{{ BUILD }}/$BUILD/g" \
    "$APP/Contents/Info.plist"

cp "$BINARY" "$APP/Contents/MacOS/frigicom"

# `cp -RL` throughout: the staged trees are threaded with symlinks into the
# store, and a symlink is exactly what must not survive into the image.
cp -RL "$LOGOS_MODULES_DIR"/* "$LOGOS/modules/"

if [ -n "${LOGOS_PROTOCOL_ROOT:-}" ] && [ -d "$LOGOS_PROTOCOL_ROOT/lib" ]; then
    cp -L "$LOGOS_PROTOCOL_ROOT"/lib/*.dylib "$APP/Contents/Frameworks/"
fi

# Nix installs each logos binary behind a compiled wrapper whose only job
# is to set environment variables to store paths, and the module loaders
# are wrapped *twice*. Unwrapping one level is the trap: what is left is
# still a wrapper, it still exec's the build machine's copy, and the copy
# it reaches loads a second QtCore alongside the bundle's — after which
# every module sees a null QCoreApplication and fails its init.
#
# `makeCWrapper` records its target verbatim, so the chain can be walked
# to the real executable. Every link is worth reading on the way: the
# variables the wrappers set are the only record of where Qt's plugins
# were, and different links set different ones.
chain() {
    local link="$1" target

    while [ -f "$link" ]; do
        printf '%s\n' "$link"
        target=$(strings -a "$link" 2>/dev/null |
            sed -n "s/^makeCWrapper '\([^']*\)'.*/\1/p" | head -1)
        [ -n "$target" ] || break
        link="$target"
    done
}

read_chain() {
    while read -r link; do
        [ -f "$link" ] && strings -a "$link"
    done
}

CORE=$(real "$LOGOSCORE_BIN")
CORE_CHAIN=$(chain "$CORE")
CHAINS="$CORE_CHAIN"

if [ "$(printf '%s\n' "$CORE_CHAIN" | wc -l)" -gt 1 ]; then
    cp -L "$(printf '%s\n' "$CORE_CHAIN" | tail -1)" "$LOGOS/libexec/logoscore"

    HOST=$(printf '%s\n' "$CORE_CHAIN" | read_chain |
        sed -n 's/^LOGOS_HOST_PATH=//p' | head -1)

    if [ -n "$HOST" ] && [ -e "$HOST" ]; then
        HOST_DIR=$(dirname "$(real "$HOST")")

        # Both loaders are staged: the daemon picks the Qt one when it can
        # and falls back to the plain one, and it looks them up by name.
        for name in logos_host logos_host_qt; do
            [ -f "$HOST_DIR/$name" ] || continue
            HOST_CHAIN=$(chain "$HOST_DIR/$name")
            cp -L "$(printf '%s\n' "$HOST_CHAIN" | tail -1)" \
                "$LOGOS/libexec/$name"
            CHAINS=$(printf '%s\n%s' "$CHAINS" "$HOST_CHAIN")
        done
    else
        echo "  ! no LOGOS_HOST_PATH in the wrapper; modules may not load" >&2
    fi

    # Qt finds plugins through the environment, and the wrappers are the
    # only record of which directories those were — read them back out
    # rather than hardcoding store hashes that change on every rebuild.
    #
    # Only two families are taken. They are loaded by name at runtime, so
    # the closure walk cannot discover them: `tls` is how QtNetwork reaches
    # openssl and `networkinformation` is how it notices the link come and
    # go. Nothing else here is plugin-driven — daemon and modules between
    # them link QtCore, QtNetwork and QtRemoteObjects and nothing more —
    # and staging the rest of the tree drags the entire Qt Quick stack in
    # behind it: fifty-odd frameworks that never get loaded.
    printf '%s\n' "$CHAINS" | read_chain |
        grep -E '/lib/qt-6/plugins$' | sort -u | while read -r dir; do
        for kind in ${FRIGICOM_QT_PLUGINS:-tls networkinformation}; do
            # Several of the listed directories carry the same family, so
            # the copies overlap; store files arrive read-only, and the
            # second pass cannot overwrite the first without this.
            if [ -d "$dir/$kind" ]; then
                mkdir -p "$LOGOS/qt-plugins/$kind"
                cp -RL "$dir/$kind"/* "$LOGOS/qt-plugins/$kind/"
                chmod -R u+w "$LOGOS/qt-plugins/$kind"
            fi
        done
    done
else
    cp -L "$CORE" "$LOGOS/libexec/logoscore"
fi

cat > "$LOGOS/bin/logoscore" <<'WRAPPER'
#! /bin/sh
# Stands in for the nix wrapper: same three variables, pointed inside the
# bundle. The linker paths are cleared because a machine that also has a
# dev shell would otherwise resolve the build tree's dylibs over ours.
here=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
unset LD_LIBRARY_PATH DYLD_LIBRARY_PATH DYLD_FRAMEWORK_PATH
# Every module is loaded in its own host process, and the Qt variant is
# the one that brings a QCoreApplication with it. Naming the plain host
# here is not a smaller mistake than naming none: the loader goes looking
# for `logos_host_qt` elsewhere, finds the path it was compiled against,
# and runs a host out of the build machine's store — which then loads a
# second copy of QtCore alongside the bundle's, leaving the module to see
# a null QCoreApplication and fail its init.
if [ -x "$here/libexec/logos_host_qt" ]; then
    export LOGOS_HOST_PATH="$here/libexec/logos_host_qt"
else
    export LOGOS_HOST_PATH="$here/libexec/logos_host"
fi

export QT_PLUGIN_PATH="$here/qt-plugins"
exec "$here/libexec/logoscore" "$@"
WRAPPER

chmod +x "$LOGOS/bin/logoscore"
chmod -R u+w "$APP"

# ----------------------------------------------------------------- relocate

step "rewriting load commands off the nix store"
# Origins land outside the stage: everything under it is handed to
# hdiutil verbatim, so scratch written there ships inside the image.
ORIGINS_FILE="target/$PROFILE/origins.txt"

python3 scripts/macos-bundle-libs.py "$APP" "$ORIGINS_FILE" || \
    die "relocation left the bundle dependent on this machine"

# ------------------------------------------------------------------ declare

# What is in the image, said out loud. One of the four modules is our fork,
# so an image that does not say which is one nobody can reason about: a
# tester cannot tell our bug from an upstream one, and neither can we from
# their report. The terms have to travel too — frigicom is GPL-3.0-or-later
# and the modules are MIT/Apache-2.0, and all of those oblige us to convey
# the licence with the binary.
step "declaring what the bundle contains"

RESOURCES="$APP/Contents/Resources"
TREE=$(dirname "$LOGOS_MODULES_DIR")

cp LICENSE "$RESOURCES/LICENSE"

if [ -d "$TREE/licenses" ]; then
    cp -RL "$TREE/licenses" "$RESOURCES/licenses"
    chmod -R u+w "$RESOURCES/licenses"
else
    echo "  ! no licence texts beside $LOGOS_MODULES_DIR" >&2
fi

MODULES_DIR="$LOGOS_MODULES_DIR" \
PROVENANCE="$TREE/provenance.json" \
ORIGINS="$ORIGINS_FILE" \
REPO=$(sed -n 's/^repository = "\(.*\)"/\1/p' Cargo.toml | head -1) \
python3 - "$RESOURCES" <<'DECLARE'
import json, os, sys, textwrap

resources = sys.argv[1]
provenance = os.environ["PROVENANCE"]
modules_dir = os.environ["MODULES_DIR"]

def manifest_versions():
    found = {}
    for name in sorted(os.listdir(modules_dir)):
        path = os.path.join(modules_dir, name, "manifest.json")
        if os.path.isfile(path):
            with open(path) as handle:
                found[name] = json.load(handle).get("version", "unknown")
    return found

versions = manifest_versions()
lines = ["Logos modules bundled with this build", "=" * 38, ""]

# A tree built by `nix build .#modules` records where every module came
# from. A hand-staged one cannot, and saying so is the point: an
# unreproducible tree is exactly what a reader needs warning about.
if os.path.isfile(provenance):
    with open(provenance) as handle:
        recorded = json.load(handle)["modules"]

    if any(m.get("patched") for m in recorded.values()):
        lines += [
            "One of these is NOT upstream. Frigicom carries a patched",
            "chat module; behaviour you see here may not be Logos'.",
            "",
        ]

    for name in sorted(recorded):
        entry = recorded[name]
        mark = "PATCHED" if entry.get("patched") else "upstream"
        lines.append(f"  {name:<20} {entry.get('version', '?'):<8} {mark}")
        lines.append(f"      source    {entry['flake']} @ {entry['rev']}")

        if entry.get("upstream"):
            lines.append(f"      upstream  {entry['upstream']}")

        for patch in entry.get("patches", []):
            for n, chunk in enumerate(textwrap.wrap(patch, 58)):
                lines.append(f"      {'changes' if n == 0 else '':<9} {chunk}")

        lines.append("")
else:
    lines += [
        "PROVENANCE NOT RECORDED. This tree was staged by hand, so the",
        "exact source of each module below cannot be established from",
        "the build. Rebuild with `nix build .#modules` for a tree that",
        "records itself.",
        "",
    ]
    lines += [f"  {n:<20} {v}" for n, v in versions.items()] + [""]

lines += [
    "Licence texts for each module are in licenses/.",
    "",
    "Frigicom itself is GPL-3.0-or-later; the full text is in LICENSE,",
    f"and its source is at {os.environ.get('REPO') or 'the project repository'}.",
    "It is a fork of halloy (https://github.com/squidowl/halloy).",
]

with open(os.path.join(resources, "MODULES.txt"), "w") as handle:
    handle.write("\n".join(lines) + "\n")

# The dylib closure, named. These are the libraries the relocator pulled in
# beside the app; Qt in particular is LGPL-3, which is satisfied here
# because it ships as separate dylibs the user can replace.
origins = os.environ.get("ORIGINS", "")
if os.path.isfile(origins):
    with open(origins) as handle:
        components = [line.strip() for line in handle if line.strip()]

    heading = "Libraries bundled in Contents/Frameworks"
    notice = [
        heading,
        "=" * len(heading),
        "",
        "Each ships as a separate dylib, so any of them can be replaced by",
        "relinking the bundle. Qt is used under the LGPL-3 on that basis.",
        "",
    ] + [f"  {component}" for component in components]

    with open(os.path.join(resources, "THIRD-PARTY.txt"), "w") as handle:
        handle.write("\n".join(notice) + "\n")
DECLARE

# -------------------------------------------------------------------- audit

step "auditing the bundle for build-machine identity"

patterns="$(id -un) $(basename "$HOME") ${FRIGICOM_AUDIT_EXTRA:-}"
leaked=0

for pattern in $patterns; do
    [ -n "$pattern" ] || continue

    if hits=$(grep -rlF -- "$pattern" "$APP" 2>/dev/null) && [ -n "$hits" ]; then
        echo "  ! '$pattern' appears in:" >&2
        echo "$hits" | sed 's/^/      /' >&2
        leaked=1
    fi
done

[ "$leaked" -eq 0 ] || die "the bundle names the build machine; not packaging it"

# --------------------------------------------------------------------- sign

step "signing (identity: $SIGN_ID)"

# Deepest first: signing a container seals what is inside it, so anything
# signed afterwards invalidates the seal above it.
find "$APP" -type f ! -type l -print0 |
    while IFS= read -r -d '' file; do
        printf '%s\t%s\n' "$(tr -cd / <<<"$file" | wc -c)" "$file"
    done |
    sort -rn |
    cut -f2- |
    while IFS= read -r file; do
        case "$(file -b "$file")" in
            Mach-O*) codesign --force --sign "$SIGN_ID" "$file" 2>/dev/null ;;
        esac
    done

for framework in "$APP/Contents/Frameworks"/*.framework; do
    if [ -d "$framework" ]; then
        codesign --force --sign "$SIGN_ID" "$framework"
    fi
done

codesign --force --sign "$SIGN_ID" "$APP"
codesign --verify --deep --strict "$APP" || die "the signed bundle does not verify"

# ---------------------------------------------------------------------- dmg

step "packing the disk image"

ln -sf /Applications "$STAGE/Applications"

cat > "$STAGE/README.txt" <<'README'
Frigicom — a Logos chat client
==============================

Drag Frigicom.app onto the Applications shortcut, then run this once:

    xattr -dr com.apple.quarantine /Applications/Frigicom.app

That step is not optional. This build is signed ad-hoc rather than with an
Apple Developer certificate, so without it macOS refuses to open the app
and reports it as damaged. The command only strips the download flag; it
does not change the app.

Requirements: an Apple Silicon Mac running macOS 12 or later.

The app starts its own background daemon and loads the chat, delivery and
capability modules by itself; give it a few seconds on first launch. Chat
identity is regenerated every run, so conversations do not survive a
restart — that is a known limitation of the stack, not a bug in the build.
README

rm -f "$DMG"
hdiutil create "$DMG" -volname "Frigicom" -fs HFS+ -srcfolder "$STAGE" \
    -ov -format UDZO -quiet

step "done"
printf '  %s (%s)\n' "$DMG" "$(du -h "$DMG" | cut -f1)"
printf '  recipients must run: xattr -dr com.apple.quarantine /Applications/%s\n' "$APP_NAME"
