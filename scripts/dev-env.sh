#! /bin/sh
# Developer environment for the logos backend crates. Source it, don't run it:
#
#   . scripts/dev-env.sh
#
# Exports the store paths the logos/* crates build and run against. Nothing
# here is hardcoded to one machine: every path is derived from this script's
# own location, and every value can be overridden by exporting it first.
#
#   FRIGICOM_ARTIFACTS   where the nix GC roots and the staged module trees
#                        live (default: <repo>/../.gcroots). The symlinks in
#                        it pin the store paths against garbage collection.
#   LOGOS_PROTOCOL_ROOT  liblogos_protocol prefix (default: the newest
#                        logos-protocol-lib-* root under FRIGICOM_ARTIFACTS)
#   LOGOSCORE_BIN        the logoscore daemon binary
#   LOGOS_MODULES_DIR    the merged module tree (see below)
#
# `nix develop` exports the same variables from flake-built packages instead;
# use whichever you prefer.

# Locate the checkout from the path this file was sourced as: bash exposes it
# as BASH_SOURCE, zsh as $0. Anywhere else (dash, and POSIX_ARGZERO shells)
# that is the shell's own name, so fall back to git and then to the cwd —
# each candidate is accepted only if this script is really under it.
_frigicom_self="${BASH_SOURCE:-$0}"
_frigicom_root=""
if [ -f "$_frigicom_self" ]; then
    _frigicom_root=$(CDPATH= cd -- "$(dirname -- "$_frigicom_self")/.." 2>/dev/null && pwd)
fi
[ -f "${_frigicom_root:-/nonexistent}/scripts/dev-env.sh" ] || \
    _frigicom_root=$(git rev-parse --show-toplevel 2>/dev/null)
[ -f "${_frigicom_root:-/nonexistent}/scripts/dev-env.sh" ] || _frigicom_root="$PWD"
unset _frigicom_self

_frigicom_artifacts="${FRIGICOM_ARTIFACTS:-$_frigicom_root/../.gcroots}"
if [ -d "$_frigicom_artifacts" ]; then
    _frigicom_artifacts=$(CDPATH= cd -- "$_frigicom_artifacts" && pwd)
else
    echo "dev-env: WARNING: no artifacts dir at $_frigicom_artifacts; \
export FRIGICOM_ARTIFACTS (or the three LOGOS_* variables) yourself"
fi
# A tree built by `nix build .#modules` is preferred over anything staged
# by hand, because it is the only one that records where its modules came
# from — which is what a disk image built from it has to declare, one of
# the four being our fork. See docs/packaging-macos.md.
_frigicom_built="$_frigicom_root/result-modules"
unset _frigicom_root

# Newest logos-protocol-lib-* root wins. `find` rather than a glob: zsh
# aborts a sourced script when a glob matches nothing. The symlink is
# resolved to its store path because that is what build.rs bakes into the
# rpath, and keeping it stable keeps the build cache warm.
if [ -z "${LOGOS_PROTOCOL_ROOT:-}" ]; then
    _frigicom_lib=$(find "$_frigicom_artifacts" -maxdepth 1 \
        -name 'logos-protocol-lib-*' 2>/dev/null | sort | tail -n 1)
    if [ -n "$_frigicom_lib" ] && [ -d "$_frigicom_lib/lib" ]; then
        LOGOS_PROTOCOL_ROOT=$(CDPATH= cd -- "$_frigicom_lib" && pwd -P)
    fi
    unset _frigicom_lib
fi
if [ -n "${LOGOS_PROTOCOL_ROOT:-}" ]; then
    export LOGOS_PROTOCOL_ROOT
else
    echo "dev-env: WARNING: no logos-protocol-lib-* under $_frigicom_artifacts; \
the live feature will not link — export LOGOS_PROTOCOL_ROOT"
fi

# The daemon that `nix build .#modules` staged wins over any GC root,
# because it is the one the modules in that same tree were built against.
# Taking the two from different places is how a pin bump moves the modules
# and leaves the daemon behind — everything still starts, and the mismatch
# only shows up as behaviour nobody can account for.
if [ -z "${LOGOSCORE_BIN:-}" ] && [ -x "$_frigicom_built/bin/logoscore" ]; then
    LOGOSCORE_BIN="$_frigicom_built/bin/logoscore"
fi
: "${LOGOSCORE_BIN:=$_frigicom_artifacts/logos-logoscore-cli/bin/logoscore}"
export LOGOSCORE_BIN

# The daemon needs all three modules in ONE directory: chat_module,
# delivery_module, and capability_module. No single upstream output stages
# them together — logos-chat-module's `.#install` ships chat_module alone
# and logoscore-cli ships capability_module alone — so the modules-live*
# trees are merged by hand, and they are what the live tests run against.
#
# `nix build .#modules` is the source of truth: it stages all five modules
# and the daemon they were built against, and records where each came from.
# Everything below it is a hand-merged fallback from before that existed,
# kept only so an old checkout still runs. They have no provenance record
# and no matching daemon; prefer the built tree.
if [ -z "${LOGOS_MODULES_DIR:-}" ]; then
    for _frigicom_modules in \
        "$_frigicom_built/modules" \
        "$_frigicom_artifacts/modules-live-v6" \
        "$_frigicom_artifacts/modules-live-v4" \
        "$_frigicom_artifacts/modules-live-v3" \
        "$_frigicom_artifacts/modules-live-v2" \
        "$_frigicom_artifacts/modules-live" \
        "$_frigicom_artifacts/chat-module-install/modules" \
        "$_frigicom_artifacts/logos-logoscore-cli/modules"; do
        [ -d "$_frigicom_modules" ] && break
    done
    LOGOS_MODULES_DIR="$_frigicom_modules"
    unset _frigicom_modules
fi
export LOGOS_MODULES_DIR
[ -d "$LOGOS_MODULES_DIR" ] || echo "dev-env: WARNING: no staged module tree \
under $_frigicom_artifacts; export LOGOS_MODULES_DIR to a directory holding \
chat_module, delivery_module and capability_module"
unset _frigicom_artifacts _frigicom_built

# Append the dylib dir to the dynamic linker path, once (logos-sys bakes an
# rpath, so this is belt-and-braces for tools that bypass it).
case "$(uname -s)" in
    Darwin) _frigicom_lib_var="DYLD_LIBRARY_PATH" ;;
    *) _frigicom_lib_var="LD_LIBRARY_PATH" ;;
esac
eval "_frigicom_lib_path=\"\${$_frigicom_lib_var:-}\""
case ":$_frigicom_lib_path:" in
    *":${LOGOS_PROTOCOL_ROOT:-}/lib:") ;;
    *":${LOGOS_PROTOCOL_ROOT:-}/lib:"*) ;;
    ::) [ -n "${LOGOS_PROTOCOL_ROOT:-}" ] && \
        eval "export $_frigicom_lib_var=\"\$LOGOS_PROTOCOL_ROOT/lib\"" ;;
    *) [ -n "${LOGOS_PROTOCOL_ROOT:-}" ] && \
        eval "export $_frigicom_lib_var=\"\$_frigicom_lib_path:\$LOGOS_PROTOCOL_ROOT/lib\"" ;;
esac
unset _frigicom_lib_path

echo "dev-env: LOGOS_PROTOCOL_ROOT, LOGOSCORE_BIN, LOGOS_MODULES_DIR=$LOGOS_MODULES_DIR; ${LOGOS_PROTOCOL_ROOT:-<unset>}/lib on $_frigicom_lib_var"
unset _frigicom_lib_var
