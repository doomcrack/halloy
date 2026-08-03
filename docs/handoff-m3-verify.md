# Verify the Intel (x86_64-darwin) changes haven't broken arm64 (M3)

Three repos changed to bring the Logos module tree + live backend up on Intel.
Each change is a one-liner; two of them touch code paths that also run on
`aarch64-darwin`, so the M3 needs to confirm arm64 still builds. Paste the block
below to an agent on the M3.

## The changes (and their arm64 blast radius)

| repo | branch / rev | change | affects arm64? |
|---|---|---|---|
| `doomcrack/nix-bundle-lgx` | `main` @ `ae10e36` (merged) | darwin variant token `darwin-amd64` → `darwin-x86_64` | **No** — only the `else` (x86_64) branch; arm64 stays `darwin-arm64`. |
| `doomcrack/logos-delivery` | `fix/darwin-libcxx` @ `2e75b99` | add `-lc++` to the Nim link **on `isDarwin`** | **Yes** — `isDarwin` includes arm64. This is the one to watch. |
| `doomcrack/halloy` (frigicom) | `intel-x86_64-support` | `.#modules` licence loop → bash array; new `scripts/nbl.sh`, docs, `upstream/issues/` | **Maybe** — arm64 *includes* blockchain, so the old `for` loop worked there; the array must still work. |

## The prompt

> You are verifying that Intel-mac (`x86_64-darwin`) fixes did not break
> `aarch64-darwin`. Everything is already pushed; nothing needs merging to test.
>
> ```
> repo: git@github.com:doomcrack/halloy.git   branch: intel-x86_64-support
> ```
>
> ### Test 1 — the frigicom flake change, stock inputs (arm64, blockchain included)
> ```sh
> git fetch && git checkout intel-x86_64-support
> nix build .#modules --out-link result-modules -L --accept-flake-config
> ```
> Expect: builds; stages **five** modules on arm64 (blockchain included), each
> `variant`/`main` = `darwin-arm64-dev`. This exercises the licence-loop bash-array
> change (`flake.nix`) on the blockchain-*included* path. If it fails with a shell
> `syntax error` in `frigicom-modules`, the array rewrite regressed arm64 — report it.
>
> ### Test 2 — our two forks applied (arm64): the delivery `-lc++` is the risk
> ```sh
> ./scripts/nbl.sh fork build .#modules --out-link result-modules-fork
> ```
> This overrides `nix-bundle-lgx` → our fork (a no-op on arm64) and
> `logos-delivery` → our `-lc++` fork (**does** change arm64). Expect: builds; all
> modules `darwin-arm64-dev`. **The thing to confirm: `liblogosdelivery` still
> links on arm64 with the extra `-lc++`.** If it fails to link, the fix should be
> narrowed to `pkgs.stdenv.isDarwin && pkgs.stdenv.isx86_64` in
> `logos-delivery`'s `nix/default.nix` (so arm64 is untouched) — report the exact
> linker error either way.
>
> ### Test 3 (optional) — live backend on arm64
> `nbl.sh fork` also builds the logoscore bundle. Run the client with the **system**
> toolchain and the exported `LOGOS_*` env — **not** inside `nix develop` (its SDK
> may lack newer symbols; on Intel it was missing `AudioHardwareDestroyProcessTap`):
> ```sh
> . scripts/dev-env.sh          # or export LOGOSCORE_BIN / LOGOS_PROTOCOL_ROOT / LOGOS_MODULES_DIR by hand
> cargo run --features live
> ```
> Expect `backend ready: address …` in `~/Library/Application Support/frigicom/logs/`.
>
> ### What to report
> 1. Test 1 result (build + five modules `darwin-arm64-dev`?).
> 2. Test 2 result — specifically whether `liblogosdelivery` links on arm64 with
>    `-lc++`. If it does, the `isDarwin` scope is fine; if not, narrow to x86_64.
> 3. Any arm64 regression, with the derivation + last ~30 log lines.

## Context — measured on Intel

Full write-up in `docs/logos-modules.md` §8 and `upstream/issues/{0001,0002}`.
On Intel these three fixes produce a complete `darwin-x86_64-dev` tree and a live
backend (`backend ready`, delivery joins the logos.dev fleet, 0 crashes).

## The reply — measured on the M3

Answered 2026-08-02 on `aarch64-darwin`, macOS 14.5 (23F79), against
`intel-x86_64-support` `47c5da4f`. **No arm64 regression in any of the three
changes.** The `-lc++` is safe at `isDarwin` scope and must *not* be narrowed.

### 1. Test 1 — stock inputs: pass

`nix build .#modules` builds. Five modules staged (blockchain included), every
`variant` and every `manifest.json` `main` key = `darwin-arm64-dev`, five
directories under `licenses/`. The licence loop ran to completion on the
blockchain-*included* path, which is the case the array rewrite had to keep
working; no shell syntax error.

### 2. Test 2 — both forks applied: pass

`./scripts/nbl.sh fork build .#modules` exits 0 and stages the same five modules
at `darwin-arm64-dev`.

This was a real test rather than a formality: **arm64 had never built delivery
from source.** Test 1 substitutes delivery's prebuilt `.lgx` from the cache, so
the `-lc++` line had never been exercised on this platform at all — a failure
here could as easily have been a pre-existing from-source break as a regression.
It linked:

```
liblogosdelivery.dylib:  Mach-O 64-bit dynamically linked shared library arm64
  @rpath/librln.dylib
  /usr/lib/libc++.1.dylib (compatibility version 1.0.0, current version 1900.180.0)
```

`libc++.1.dylib` in the load commands is the flag taking effect. So
`pkgs.stdenv.isDarwin` is the right scope in `logos-delivery`'s
`nix/default.nix`; narrowing it to `isDarwin && isx86_64` would be scope the fix
does not need.

### 3. The check this handoff did not ask for — the merge is a no-op

`intel-x86_64-support` branches from `b00501fc` and so does **not** carry
`frigicom`'s tip `4f9c0694` (logoscore 0.2.2, `bin/logoscore` staged into
`.#modules`, a 2747-line `flake.lock` rewrite, `dev-env.sh`). Test 1 therefore
exercises the *old* lock, which is not what merging produces. Merged in a
throwaway worktree instead:

- the merge is **clean** — `flake.nix` and `docs/logos-modules.md` auto-merge on
  disjoint hunks;
- `nix build .#modules` on the merged tree is **`diff -r` identical** to what
  `frigicom` builds today: same five modules, same licences, same
  `bin/logoscore`, same `provenance.json`;
- `nbl.sh`'s jq input-path walk still resolves against the 0.2.2 lock
  (`nbl.sh fork eval .#…modules.drvPath` produces a derivation).

A byte-identical output is the strongest available form of "does not break
arm64": the licence-loop change provably alters nothing here.

Test 3 (live backend) was not run, and is redundant given the above — the arm64
runtime tree is bit-for-bit the one already in use.

### 4. Three things back to the Intel side

1. **The Intel measurements predate the 0.2.2 bump.** §8 records them against
   `b00501fc`. The merge is inert on arm64 but rewrites the lock wholesale, so
   `nbl.sh fork build .#modules` wants re-running on Intel *after* the merge,
   not before.
2. **`--out-link result-modules-fork` becomes a skew trap after the merge.**
   `4f9c0694` made `dev-env.sh` prefer `$root/result-modules/bin/logoscore` and
   look nowhere else. On Intel there is no stock `result-modules` to build, so a
   fork tree parked under a different name leaves the daemon resolving from the
   `.gcroots` GC root — fork-built modules paired with an unrelated daemon,
   which is precisely the skew `4f9c0694` exists to make inexpressible.
   `nbl.sh`'s own usage line already says `--out-link result-modules`; Tests 2
   and 3 above should say the same.
3. **Both commits on this branch are `doomcrack <local@local>` and unsigned**,
   against a history of `doomcrack <qtx8zpvd66@privaterelay.appleid.com>` with
   `commit.gpgsign = true`. The diff itself is identity-clean. Worth correcting
   the author email and signing when this branch lands.

The upstream `nix-bundle-lgx` PR is arm64-safe by inspection — only the `else`
(x86_64) arm of the variant ternary changes — and empirically, since Test 2
built the whole tree through that fork.

### Environment note

Test 2 failed once on `No space left on device` before it reached any linking,
which is worth separating from the result above: `halloy/target` had grown to
**48 GB**, 24 GB of it `debug/incremental`. `RESUME.md`'s "~7 GB" figure is
stale. Dropping `target/debug/incremental` and the disk-corrupted
`~/.cache/nix/eval-cache-v6` freed enough to finish. `CARGO_INCREMENTAL=0` in
`.cargo/config.toml`, already suggested there, looks less optional than it did.
