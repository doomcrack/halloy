# Keeping up with the Logos stack

Frigicom sits on five upstream components that are all under active
development, plus two forks of our own. Bumping them is not occasional
housekeeping — it is a routine part of the work, and the cost of doing it
badly is a stack that "still starts" while quietly disagreeing with itself.

This is the procedure, and the findings that shaped it. It was written by
doing one: `logoscore` from a moving branch to tag `0.2.2`.

## What we depend on

| pin | what it is | ours? |
|---|---|---|
| `logoscore` | the daemon **and** `capability_module` | no |
| `logos-protocol` | `liblogos_protocol`, the dylib the app links | no |
| `chat-module` | `chat_module` | **fork** |
| `delivery-module` | `delivery_module` | no |
| `blockchain-module` | `blockchain_module` | no |
| `keystore-signer-module` | `keystore_signer` | no |
| `libchat` (via chat-module) | the chat core | **fork** |

Two of them are one pin serving two purposes, and that is where the first
trap is.

## The procedure

**1. Record a baseline before touching anything.** Pins and revs, the
daemon's own reported version, `cargo test` counts for both workspaces, and
a live run reaching `backend ready`. "What broke" is only answerable
against something written down.

**2. Move one pin.** Not several. When something breaks two pins later you
will want to know which one did it.

**3. Prefer a tag to a branch.** A branch head moves under you between
builds, so two people — or the same person on two days — get different
stacks from the same `flake.nix`. Every finding recorded against a branch
pin has a silent expiry date.

**4. Rebuild the tree, not just the thing you bumped.**

```sh
nix build .#modules --out-link result-modules
. scripts/dev-env.sh
cargo test --workspace          # both workspaces
cargo build --features live
```

**5. Run it.** Unit tests do not exercise the daemon contract at all. The
bar is `backend phase: Online` and `backend ready: address …` in
`~/Library/Application Support/frigicom/logs/`, plus whatever the bump was
*for* — for a keystore-affecting bump, that the address is the same across
two runs against one home directory.

**6. Write down what changed**, in `logos-modules.md` if it is a fact about
the stack, here if it is a fact about maintaining it.

## What the `0.2.2` bump actually taught

### One pin moved two things, and only one of them moved

`logoscore` supplies both the daemon binary and `capability_module`.
Bumping it moved `capability_module` to `8720885` — and left the daemon at
`pre-release-679a9af`, because `dev-env.sh` took the daemon from a
hand-made GC root while the modules came from the flake.

Everything still started. A module set built against one daemon version,
running on another, is exactly the failure that produces behaviour nobody
can account for.

**Fixed structurally rather than by remembering:** `.#modules` now stages
`bin/logoscore` beside `modules/`, and `dev-env.sh` prefers it. The tree
that carries the modules carries the daemon they were built against, so
the two cannot drift apart again.

### Check which way a skew points before closing it

Applying the same reasoning to `liblogos_protocol` found the reverse:

| source | version |
|---|---|
| the GC root `dev-env.sh` uses | **0.2.0** |
| the rev the flake pins | **0.1.0** |

Here the hand-staged artifact is *newer*. "Unify on the flake" would have
been a silent downgrade of the ABI the app links against.

**Left open deliberately.** Closing it means relinking against a different
protocol version, which deserves its own pass with its own baseline rather
than riding along with a daemon bump. It is recorded in `flake.nix` at the
input, so the next person meets the warning before the bug.

A live consequence worth knowing: **`nix develop` and `. scripts/dev-env.sh`
are documented as equivalent and are not.** The dev shell exports the
flake's 0.1.0; the script exports the GC root's 0.2.0.

### A tag can be older than the branch you were on

Tag `0.2.2` is dated *before* the default-branch rev we had pinned. "Bump
to the latest release" and "move forward in time" are different operations.
Check dates, and do not assume a tag supersedes a branch head.

### Two repos can carry the same-named commit and mean different things

The `logoscore` flake input rev (`514903de`) and the version the daemon
binary reports (`679a9af`) are commits in *different* repositories. An
early comparison in this session — "we are 9 commits behind the fix" — was
made in the inner repo and cannot be checked against the outer pin. When
comparing revs, confirm which repository each one belongs to first.

### `cargo update` is not the tool

A blanket `cargo update` inside `logos-chat-module` touched **358**
packages to move one dependency. Targeted updates plus a plain resolve did
the same job in **6**:

```sh
cargo update -p libchat -p logos-generic-chat -p logos-account
cargo metadata --format-version 1 >/dev/null   # adds a genuinely new dep
```

### Our patches are debts, and each needs an exit

Every fork carries fixes that upstream may land independently. Re-check on
each bump, and delete ours the moment upstream's is real — a patch kept
past its usefulness is a merge conflict with no benefit.

| ours | drop it when |
|---|---|
| libchat duplicate-Welcome fix | upstream skips a duplicate rather than aborting |
| delivery config / online-state fixes | upstream sends only accepted keys and gates `online` on a started node |
| `from_seed` constructors | libchat can construct an identity from stored material |
| the keystore identity derivation | `PERSISTENCE_ENABLED` is on and upstream persistence works |

## Two standing constraints

- **`keystore_signer` must stay a single caller.** A cross-module `sign`
  returned a *different* caller's argument before `0.2.2-RC1`. On 0.2.2
  that is fixed, but only `chat_module` calls it today and a second caller
  should not be added without re-testing that specifically.
- **`blockchain_module` has no `x86_64-darwin` build.** Its circuits
  dependency publishes no output for that platform, so `.#modules` stages
  four modules there rather than failing to evaluate. See
  `handoff-intel-mac.md`.

## What the bump cost, for calibration

`logoscore` branch → tag `0.2.2`: two flake edits, one lock update, one
tree rebuild, two structural fixes to how the daemon is resolved. **302
tests green, live run reaching `Online`, address stable across restarts,
no application code changed.**
