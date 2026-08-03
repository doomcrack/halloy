#!/usr/bin/env bash
# nbl.sh — build/eval frigicom with a chosen nix-bundle-lgx source.
#
# nix-bundle-lgx stamps the platform variant on every Logos module .lgx. On
# x86_64-darwin, logos-co's rev stamps `darwin-amd64` while `lgpm` resolves the
# host as `darwin-x86_64`, so `lgpm install` rejects every module and the live
# backend cannot build on Intel. doomcrack's fork fixes that one line. See
# docs/logos-modules.md §8 and upstream/issues/0001.
#
# nix-bundle-lgx is pinned ~181x transitively through a cyclic graph
# (logos-module-builder <-> logos-standalone-app <-> logos-capability-module),
# so there is no single-flag override and a flake.lock rewrite is reverted by
# re-locking. This script derives every nix-bundle-lgx input path from
# flake.lock and emits one `--override-input <path> <ref>` per path.
# `--override-input` is honoured (not reverted), so this is the reliable toggle.
#
# Usage:
#   scripts/nbl.sh fork     build .#modules --out-link result-modules
#   scripts/nbl.sh upstream build .#modules
#   scripts/nbl.sh fork     develop            # etc. — any nix subcommand
#
#   fork     -> override nix-bundle-lgx to the doomcrack fork (Intel fix)
#   upstream -> no overrides; use the committed flake.lock as-is (logos-co)
#
# Override the fork ref if needed:  NBL_FORK_REF=path:$HOME/frigicom-fix/nix-bundle-lgx scripts/nbl.sh fork ...
#
# NOTE: the first `fork` build recompiles a little from source (overriding
# nix-bundle-lgx re-resolves its own inputs, which are not in the binary cache);
# it caches afterwards. `upstream` is always pure cache/substitute.
#
# `fork` composes BOTH Intel fixes: nix-bundle-lgx (variant, issue #0001) and
# logos-delivery (libc++ link, issue #0002). With both, `fork build .#modules`
# produces a complete darwin-x86_64-dev tree (all four modules). Verified
# 2026-08-02: FULL_BUILD_EXIT=0, every module variant/main = darwin-x86_64-dev.
set -euo pipefail

# Merged fix on doomcrack/nix-bundle-lgx main (line 26: darwin-x86_64).
NBL_FORK_REF="${NBL_FORK_REF:-github:doomcrack/nix-bundle-lgx/ae10e36da403832322df0d66e5f255b961c02b7a}"

# logos-delivery with the darwin libc++ link fix (issue #0002), on
# doomcrack/logos-delivery fix/darwin-libcxx. submodules=1 to match how the
# delivery module consumes it. For fast local iteration against an already-built
# copy, override with:  NBL_DELIVERY_REF=path:$HOME/frigicom-fix/logos-delivery
NBL_DELIVERY_REF="${NBL_DELIVERY_REF:-git+https://github.com/doomcrack/logos-delivery?submodules=1&rev=2e75b9912498b5bfa5ea148aaf81d333d7ba8fbf}"

mode="${1:-}"; shift || true
if [ -z "$mode" ] || [ "$#" -eq 0 ]; then
  echo "usage: $0 <fork|upstream> <nix-subcommand> [args...]" >&2
  echo "  e.g. $0 fork build .#modules --out-link result-modules" >&2
  exit 2
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
export NIX_CONFIG="${NIX_CONFIG:-experimental-features = nix-command flakes}"

case "$mode" in
  upstream)
    exec nix "$@" --accept-flake-config
    ;;
  fork)
    : # fall through
    ;;
  *)
    echo "$0: mode must be 'fork' or 'upstream' (got '$mode')" >&2
    exit 2
    ;;
esac

# Every distinct root->nix-bundle-lgx input-name path from flake.lock.
paths_jq='
  .root as $root | .nodes as $nodes
  | ([ $nodes|to_entries[]|select(.value.locked.repo?=="nix-bundle-lgx")|.key ]) as $targets
  | def edges($k): ($nodes[$k].inputs // {}) | to_entries
      | map({name:.key, child:(if (.value|type)=="array" then .value[0] else .value end)});
    def walk($frontier; $seen; $acc):
      if ($frontier|length)==0 then $acc
      else ($frontier|map(. as $f | edges($f.node)|map({node:.child, path:($f.path+"/"+.name)}))|add // []) as $next
        | ($next|map(select(.node as $n | ($seen|index($n))|not))) as $fresh
        | walk($fresh; ($seen + ($fresh|map(.node))); ($acc + $next)) end;
    walk([{node:$root, path:""}]; [$root]; [])
  | map(select(.node as $n | $targets|index($n))) | map(.path[1:]) | unique | .[]
'

# Build the argument list (bash 3.2 compatible — no mapfile).
args=()
while IFS= read -r p; do
  [ -n "$p" ] || continue
  args+=(--override-input "$p" "$NBL_FORK_REF")
done < <(jq -r "$paths_jq" flake.lock)

if [ "${#args[@]}" -eq 0 ]; then
  echo "$0: no nix-bundle-lgx inputs found in flake.lock (nothing to override)" >&2
  exit 1
fi
nbl_count=$(( ${#args[@]} / 3 ))

# logos-delivery libc++ fix (issue #0002): a single input path.
del_paths=$(jq -r '
  .root as $root | .nodes as $nodes
  | ([ $nodes|to_entries[]|select((.value.locked.url? // "")|test("logos-messaging/logos-delivery"))|.key ]) as $targets
  | def edges($k): ($nodes[$k].inputs // {}) | to_entries
      | map({name:.key, child:(if (.value|type)=="array" then .value[0] else .value end)});
    def walk($frontier; $seen; $acc):
      if ($frontier|length)==0 then $acc
      else ($frontier|map(. as $f | edges($f.node)|map({node:.child, path:($f.path+"/"+.name)}))|add // []) as $next
        | ($next|map(select(.node as $n | ($seen|index($n))|not))) as $fresh
        | walk($fresh; ($seen + ($fresh|map(.node))); ($acc + $next)) end;
    walk([{node:$root, path:""}]; [$root]; [])
  | map(select(.node as $n | $targets|index($n))) | map(.path[1:]) | unique | .[]
' flake.lock)
del_count=0
while IFS= read -r p; do
  [ -n "$p" ] || continue
  args+=(--override-input "$p" "$NBL_DELIVERY_REF"); del_count=$((del_count+1))
done < <(printf '%s\n' "$del_paths")

echo "$0: fork mode — $nbl_count nix-bundle-lgx -> $NBL_FORK_REF; $del_count logos-delivery -> $NBL_DELIVERY_REF" >&2
exec nix "$@" --accept-flake-config "${args[@]}"
