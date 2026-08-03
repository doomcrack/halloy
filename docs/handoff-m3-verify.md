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
