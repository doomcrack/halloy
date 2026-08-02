# Handoff: build and run frigicom on an Intel Mac

Paste the block below to an agent working on an `x86_64-darwin` machine.
Everything above the line is context for whoever is handing it over.

**Why this document exists.** Frigicom has only ever been built and run on
Apple Silicon. The published disk image is `aarch64` and will not run on
Intel at all — not "may be slow", will not launch. A source build is
expected to work for chat, and is known *not* to work for blockchain.
Nothing below is guesswork about the parts marked as established; the
platform findings were measured by evaluating each module's flake for
`x86_64-darwin`.

**What is established:**

| component | `x86_64-darwin` |
|---|---|
| `chat_module`, `delivery_module`, `keystore_signer`, `logoscore` | flake output **evaluates** |
| `blockchain_module` | **no such output** — its `logos-blockchain-circuits` dependency publishes nothing for this platform |
| frigicom itself (Rust/iced) | untested on Intel; no known reason it would not build |

Evaluating is not building. Nobody has compiled any of it for Intel.

---

## The prompt

> You are bringing up **frigicom**, a Rust/iced desktop chat client for the
> Logos network, on an Intel Mac (`x86_64-darwin`). It has only ever been
> built on Apple Silicon. Your job is to get it running, and to record
> precisely what differs — the findings matter as much as the outcome.
>
> ```
> repo    git@github.com:doomcrack/halloy.git   branch: frigicom
> ```
>
> ### Ground truth you should not re-derive
>
> - **The published `.dmg` is useless to you.** It is an arm64 bundle. Build
>   from source.
> - **`blockchain_module` cannot be built on your machine.** Its
>   `logos-blockchain-circuits` dependency has no `x86_64-darwin` output, so
>   the module tree is staged *without* it on this platform — that is
>   deliberate and already handled in `flake.nix`. You get four modules, not
>   five. Chat does not depend on blockchain; what you lose is the
>   blockchain panel, not the client.
> - **One `lp_client` per process.** `lp_token_save` is a process-wide store
>   keyed by target module, so a second client for `core_service` steals the
>   first's token and both then read as dead links. Never create a second
>   gateway.
> - **The app supervises its own daemon.** Do not start `logoscore` by hand
>   and expect the app to attach to it.
>
> ### Build and run
>
> ```sh
> nix build .#modules --out-link result-modules   # four modules on Intel
> . scripts/dev-env.sh                            # prefers result-modules/modules
> cargo run --features live
> ```
>
> `live` is not a default feature — it links `liblogos_protocol` and needs
> `LOGOS_PROTOCOL_ROOT`, which `dev-env.sh` exports. For UI-only work set
> `mock = true` under `[logos]` in `config.toml` and drop `--features live`;
> that path needs no nix, no daemon and no dylib.
>
> Success looks like this in
> `~/Library/Application Support/frigicom/logs/frigicom.*.log`:
>
> ```
> backend phase: Online
> backend ready: address <8 hex chars>
> ```
>
> That takes a few seconds. If it does not arrive, read
> `~/Library/Application Support/frigicom/logos/logoscore/logoscore.log` —
> the daemon's combined log is where module failures actually surface.
>
> ### Things that have bitten us, so you can recognise them
>
> - **A module abort can take the whole daemon with it**, and the supervisor
>   respawns in ~4s opening the log with `File::create` — which *truncates
>   away the lines proving the crash*. Do not conclude from a clean log that
>   nothing died.
> - **`logoscore watch` shows nothing even when events are flowing.** Check
>   the log instead. Our own client subscribes through `lp_subscribe`
>   directly and does not use that path.
> - **The chat identity is now stable across restarts.** It is derived from
>   a key `keystore_signer` holds and never exports. If you see
>   `chat identity will not survive a restart` in the daemon log, the
>   keystore path failed and it fell back to an ephemeral identity — that
>   line is the only signal, so grep for it.
> - **Conversations still do not survive a restart.** That is upstream and
>   expected; do not chase it.
>
> ### What to report back
>
> 1. Whether `nix build .#modules` succeeds, and how long it takes. If it
>    fails, the failing derivation and the last 30 lines of its log.
> 2. Whether `cargo run --features live` reaches `backend ready`.
> 3. **The `variant` key the modules get on your platform.** On arm64 every
>    module is `darwin-arm64-dev`; check
>    `result-modules/modules/*/manifest.json` for the `main` key and
>    `result-modules/modules/*/variant`. If the builder emits something
>    other than a `darwin-x64-*` variant, or emits arm64 on your machine,
>    stop and report — the loader selects the payload by that key and a
>    mismatch is the most likely way this fails.
> 4. Anything that needed a workaround. Those go in `docs/logos-modules.md`
>    as measured findings, in the style already there: what was run, what
>    came back, what follows.
>
> Do not assume a failure is your machine. The Logos stack is early and
> under active development; a platform gap is at least as likely as a
> local problem, and is worth filing upstream. `upstream/issues/` has the
> format we use.
