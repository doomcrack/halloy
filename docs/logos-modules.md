# The Logos modules, as measured

Frigicom supervises a `logoscore` daemon and the Logos modules it hosts. Those
modules are sparsely documented upstream, and several of their behaviours are
not what their own docs or headers imply — so this file records what was
observed directly against a running daemon, rather than what was inferred from
source.

**It is the justification for a number of otherwise-arbitrary constants in the
code**: the module-status poll interval, the default log level, the bounded
scrollback, the decision to treat a crash as a first-class UI state, and the
choice of which module version to build. Code comments cite the sections below
by number. If a measurement here is ever re-taken and disagrees, the code that
depends on it should be revisited — every claim is dated by the revisions named
at the top of each section.

**What we ship: `blockchain_module` tag `0.2.0`**
(`logos-blockchain-module` @ `88d57c92`, pinning `logos-blockchain?ref=0.2.0`),
built from source and staged alongside patched
chat 0.2.1, delivery 0.1.3 and capability 1.0.0. It syncs against the public
Testnet 0.2 fleet (§2b).

Two builds are referenced below, and the distinction matters because their
contracts differ (§2):

| build | version | status |
|---|---|---|
| `master` @ `df845c6` → `logos-blockchain` @ `133570d5` | `0.0.999` | **dead end.** Cannot join any network (§6). §4/§5 were captured on it before the skew was understood. |
| **tag `0.2.0` @ `88d57c92` → `logos-blockchain` @ `c63efe64`** | **`0.2.0`** | **what we run.** Syncs against the public fleet. |

## 1. Build and identity — resolved a real skew

Building from source was the right call, and not only for patchability — it is
**mandatory**. The prebuilt release `.lgx` cannot even be installed; `lgpm`
rejects it outright:

```
Error: Package does not contain variant for platform: darwin-arm64-dev
       (package provides: darwin-arm64)
```

That is the published `0.2.0` artifact, the one the operator guide names by
filename. There is no flag to coerce it — the variant key is how the loader
selects the payload.

**This is a track mismatch, not an upstream defect — do not file it.** Our whole
stack is the pre-release/dev track: `logoscore` reports version
`pre-release-679a9af`, and all three modules we already run
(`chat_module` 0.2.1, `delivery_module` 0.1.3, `capability_module` 1.0.0) carry
`darwin-arm64-dev` as their sole `main` key. The published blockchain artifact is
from the release track and carries `darwin-arm64`. Both are internally
consistent; they simply cannot be mixed. Building from source puts blockchain on
the same track as everything else. Beyond that, the
**prebuilt release `.lgx` is unusable for us**: its manifest declares the module
name `liblogos_blockchain_module` at version `1.0.0`, with its `main` key under
variant `darwin-arm64`. Our daemon resolves `darwin-arm64-dev`. The source build
emits the key we need, as the build log states outright:

```
Adding variant darwin-arm64-dev to logos-blockchain_module-module-lib.lgx
  (main: blockchain_module_plugin.dylib)
```

Verified manifest of our build:

| field | value |
| --- | --- |
| `name` | `blockchain_module` |
| `version` | `0.0.999` |
| `type` | `core` |
| `dependencies` | `[]` |
| `main` keys | `['darwin-arm64-dev']` |

Consequence: the module must be addressed as **`blockchain_module`**, and
`dependencies` is empty — unlike `chat_module`, which declares
`['delivery_module']` and auto-loads it.

Binary checks both passed: `strings` finds **no** `LOGOS_BLOCKCHAIN_CIRCUITS`,
`LOGOS_MODULE_PATH` or `witness_generator` references (no runtime circuit
lookup to provision), and `otool -L` shows `qtbase-6.9.2` /
`qtremoteobjects-6.9.2` — the identical store paths our live chat plugin links.

## 2. The contract — and it is SMALLER on the version we must actually run

This is the finding that most affects the UI, and it inverts the obvious
assumption. A newer build advertises "29 methods / 3 events", which looks like
the richer surface to target. In fact the version we are obliged to run — tag `0.2.0`, the only one that can join the testnet
(§6) — has the **smaller** surface:

| | `master` (0.0.999) | **tag `0.2.0`** (what we ship) |
|---|---|---|
| methods | 29 | **24** |
| events | 3 — `newBlock`, `processedBlock`, `libBlock` | **1 — `newBlock` only** |
| `get_time_info` | present | **ABSENT** |
| `get_cryptarchia_info` keys | `mode, height, slot, tip, lib, lib_slot` | `mode, height, slot, tip, lib` (**no `lib_slot`**) |

Also absent at `0.2.0`: `get_block_events`, `submit_signed_transaction`,
`wallet_fund_tx`, `get_channel_state`. Present and shared: `start`, `stop`,
`generate_user_config`, `get_cryptarchia_info`, `get_block`, `get_blocks`,
`get_transaction`, `get_peer_id`, the `wallet_*` family, and the key-management
and migration calls.

Verified by calling `module-info` against each build. Two direct consequences:

1. **The status strip cannot use `get_time_info`.** The obvious way to render
   `Bootstrapping · slot N/M (p%)` is `get_time_info().current_slot` as the
   denominator. That method does not exist here, and calling it returns
   `{"code":"METHOD_FAILED"}`.
2. **Only `newBlock` may be watched.** Registering `processedBlock` or `libBlock`
   watches must be conditional on what `getModuleInfo` actually reports, not
   hardcoded, or the same code registers dead watches or misses live ones
   depending only on which build happens to be staged.

### The denominator is computable client-side, and it checks out

The network's expected head slot is derivable without any RPC, because slot time
is fixed and public:

- `slot_duration: '1.000000000'` — one second per slot
  (`nodes/node/binary/src/config/deployment/settings.yaml:163` @ `0.2.0`)
- `genesis_time: "2026-06-30T08:30:00Z"`, `chain_id: "testnet-0.2.0"`
  (`deployment/ceremony/genesis/testnet/inscribe.yaml` @ `0.2.0`)

So `expected_slot = (now - genesis_time) / slot_duration`, and progress is
`info.slot / expected_slot`.

Sanity-checked against a live node: at wall clock ≈ `2026-08-01T04:00Z` the
formula gives ≈ **2,748,600** and the node reported **2,748,893** — agreement to
within ~0.01%. Good enough to drive a progress bar. Note the constants are baked
into the deployment settings, so they must be hardcoded alongside the module
pin and re-checked whenever the pinned tag changes.

## 2b. Sync works, and `Bootstrapping` is not a stall

Against tag `0.2.0` with the operator-guide peers, the node syncs for real:

```
Initial Block Download completed successfully
Initial Block Download completed. Starting Prolonged Bootstrap Period before going online.
```

`get_cryptarchia_info` then climbs steadily —
`height 49539 → 64846 → 79992 → 91248` over ~90 seconds, with `slot` tracking
real time — and `mode` reads `Bootstrapping`.

**`mode` does eventually reach `Online` — measured at ~1400 s (≈23 minutes)**
from `start`, at `height 91379 / slot 2752245`.

That number matters because the config's
`bootstrap.prolonged_bootstrap_period: '3600.000000000'` invites the wrong
inference. It is a **ceiling, not a fixed wait**: the node went online in well
under half of it, once it was satisfied it had caught up. Do not hardcode an
hour anywhere, and do not treat "still `Bootstrapping` after N minutes" as a
fault condition for any N below 60.

**The UI must not present `Bootstrapping` as stuck.** It is a normal state
lasting tens of minutes on a cold start, during which the node is working
correctly. A status strip showing only the word "Bootstrapping" for 23 minutes
reads as a hang. Show the height/slot progress alongside it — that is live,
obviously moving, and is the honest signal that work is happening.

### `newBlock` events fire — and during catch-up they flood

Confirmed from the daemon log: **1158** occurrences of
`ModuleProxy: forwarding event "newBlock" as Qt signal` during the sync above.
The event path works and needs no special handling to enable.

Two things follow for the UI:

- **The CLI `watch` command is not a valid test of it.** `logoscore watch
  blockchain_module --event newBlock` returned nothing across 75 seconds during
  which the log shows blocks arriving and height advancing. Do not conclude from
  a silent `watch` that events are broken — check the log instead. (This is
  consistent with the `watchModuleEvents` defects already filed as issue #6.)
  Our own client subscribes through `lp_subscribe` directly and does not go
  through this path.
- **Coalesce block events during bootstrap.** 1158 events arrived over a few
  minutes of catch-up, versus roughly one per produced block once current. A UI
  that repaints or appends per event will thrash during initial sync. Treat
  `newBlock` as a rate to sample, not a stream to render — the status strip
  wants height/slot polled on a timer, and the event is better used as a "there
  is progress" pulse.

One log line is misleading and worth not chasing: `ibd: Skipping IBD as no peers
are configured` still appears even with four peers configured and syncing. It
refers to the separate `bootstrap.ibd.peers` list, not `initial_peers`; the
actual catch-up runs through tip polling (`tip poll: enqueued peer tip for
catch-up`). Both fields exist in the generated config and they are not the same
knob.

## 3. The bring-up sequence that works

```
logoscore load-module blockchain_module
logoscore call blockchain_module generate_user_config '<json args>'   # returns the config PATH
logoscore call blockchain_module start '<that path>' ''               # deployment is "" by design
logoscore call blockchain_module get_cryptarchia_info
```

Two constraints found the hard way, both of which the UI must encode:

- **`generate_user_config` is once-only.** A second call against an existing
  config fails with `User configuration file exists. Use `update` command.`
  (`nodes/node/binary/src/cli/config/init.rs:45`). And `update_user_config`
  is **not** the escape hatch — its signature is
  `(user_config_path, keystore_path)`, i.e. keystore rotation, with no way to
  change peers. **So the app must pass every setting it cares about on the
  first generate, and skip generate entirely when the config already exists.**
- **The instance directory must exist**, created at module load. Deleting it
  and regenerating fails with `No such file or directory (os error 2)`
  (`init.rs:56`). Never hand-manage that directory.

Passing `"use_persistence_paths": true` routes state under the instance
directory (`<state>/data/blockchain_module/<instance-id>/`) instead of the
process CWD. **This is the clean answer to the CWD-relative-state problem** that
§10.3 proposed solving by anchoring the daemon's `current_dir`/`HOME` per the
operator guide. The flag is better: it is per-module and needs no supervisor
change. The supervisor anchoring is still worth doing for the other modules, but
it is no longer load-bearing for blockchain.

**If the supervisor anchoring is implemented, note the ordering trap.**
`logos/daemon/src/supervisor.rs` sets neither `.current_dir()` nor `.env()`, so
the daemon inherits the app's CWD — which is `/` when the app is launched from
Finder, meaning any module writing to a relative path today writes somewhere
arbitrary. The obvious fix, `.current_dir(config_dir)`, is **not safe as a
one-liner**: `Artifacts::logoscore_bin` and `Artifacts::modules_dir` are built
with `PathBuf::from` over environment variables
(`logos/daemon/src/locate.rs:53,65`) and so may be relative. Setting the child's
working directory would resolve them against the new CWD and break the spawn.
Canonicalize both paths first, then set `current_dir`.

## 4. Log routing — the whole basis of the monitor pane

This is the part that most needed real data, and the answer is better than
assumed: **every module line is reliably attributable to its module**,
but there are **two envelope forms**, not one, and the node's real output is in
the form the doc did not anticipate.

```
daemon    := <freeform>                                            # 3 lines, daemon's own
qt_line   := "[" ts "] [" level "] [logos] [" module "] " payload  # Qt logging bridge
stdio_line:= "[" ts "] [" stream "] [" module "] " payload         # subprocess stdio passthrough
```

Observed vocabulary over one session (2186 lines):

| form | count | discriminant values |
| --- | --- | --- |
| `stdio` | 2032 | `stream` ∈ `out` (never saw `err`) |
| `qt` | 151 | `level` ∈ `info` (146), `warning` (2), `critical` (3) |
| daemon | 3 | — |

Attribution per module: `stdio` — delivery 1976, blockchain 56; `qt` — chat 58,
blockchain 30, capability 29, delivery 26.

**The payload is a third grammar, and it differs per module.** Three
sublanguages, all confirmed:

- **blockchain (stdio)** — Rust `tracing`:
  `2026-08-01T03:27:38.163672Z  INFO logos_blockchain::storage: <msg>`
  Levels seen: `INFO` 17, `WARN` 20, `ERROR` 18.
- **delivery (stdio)** — nim/waku, abbreviated level first, local-offset
  timestamp: `INF 2026-07-31 21:32:59.363-06:00 <msg>`
  Levels: `INF` 1302, `DBG` 753, `ERR` 557, `WRN` 8. **100% of delivery's 1976
  lines matched this shape** — no exceptions to handle.
- **all modules (qt)** — freeform Qt/spdlog, e.g.
  `ModuleProxy: callRemoteMethod "get_cryptarchia_info" args: QList()`.

Consequences for the UI:

1. **Two-stage parse.** Strip the envelope to get `(module, severity_hint)`,
   then apply the module's own sublanguage for the real level. A line's true
   severity is in the payload, not the envelope — every one of blockchain's
   `ERROR` lines arrived under envelope `[out]`, and its crash arrived under
   `[info]`. **Colouring by envelope alone would be actively wrong.**
2. **ANSI must be stripped.** 56 lines carried SGR codes; the tracing output is
   coloured. Strip before parsing and re-colour ourselves.
3. **Volume is wildly asymmetric.** Delivery emitted ~35x blockchain's output,
   1976 lines in a few minutes, and 38% of it `DBG`. A bounded ring buffer and
   a default level filter are not optional; they are the difference between a
   usable pane and a firehose.

A sanitized 55-line fixture covering every shape above is captured at
`halloy/src/testkit/fixtures/module_logs.txt`.

## 5. Lifecycle — a failed start kills the module

Observed, and important enough to correct the record: when initial block
download fails, the module does **not** shut down gracefully. It aborts.

```
[out]      ... Initial Block Download failed: AllPeersFailed(AllPeersFailed).
               Initiating graceful shutdown. Retry with different bootstrap peers
[info]     ... panicked at library/std/src/thread/local.rs:428:25
[info]     ... thread panicked while processing panic. aborting.
[critical] ... FATAL: module 'blockchain_module' crashed (signal 6). Backtrace ...
[critical] ... Module process crashed: blockchain_module
```

The source confirms the first half is deliberate. `services/chain/chain-network/src/lib.rs:305-321`
handles `AllPeersFailed` by calling `overwatch_handle.shutdown()`, which tears
down **every** service in the runtime — not just chain-network. That is why all
RPC dies, not merely chain queries. There is no retry, no backoff, and no config
toggle guarding it.

The second half is not deliberate: during that teardown something reads a
`thread_local!` after its destruction (`std/thread/local.rs:428`), and the
resulting panic fires while a panic is already being processed, so std aborts
unconditionally. Afterwards `status` reports the module `not_loaded` — the module
loader's `onTerminated` callback marking it so — and every RPC fails with
`RPC_FAILED`.

Three consequences:

- **`start` is asynchronous and can fail long after it returns success.** Our
  `start` call returned `{"success": true}`; the abort came ~15s later. The UI
  cannot treat the call's return as "running" — it must watch state.
- **`crashed` is a first-class UI state**, distinct from `stopped`. The daemon
  hands it to us unambiguously on the `[critical]` line, so we should render it
  and offer a restart rather than silently showing an idle module.
- **This is an upstream bug worth filing**: a recoverable network condition
  (no reachable peers) should not abort the process. The panic-during-panic
  also means the real error is partly lost.

## 6. Why `master` could not sync — RESOLVED, and why the tag is mandatory

Connectivity is **not** the problem, which resolves an open question from
§12: the node dialled all four operator-guide bootstrap peers over QUIC from
behind NAT and every one answered. What fails is protocol negotiation:

```
peer does not support /logos-blockchain/chainsync/X.Y.Z
bootstrap::ibd: failed to fetch tip from PeerId("12D3KooW..."): channel
bootstrap::ibd: no configured peer returned a tip this round
Initial Block Download failed: AllPeersFailed(AllPeersFailed)
```

**Resolved: `X.Y.Z` is a literal, un-substituted placeholder, and building from
`master` was the mistake.** `settings.yaml` is `include_bytes!`d into the binary
(`nodes/node/binary/src/config/deployment/mod.rs:15`), and on `master` six
protocol identifiers still read `X.Y.Z` — verified first-hand at lines 5, 17, 18,
19, 38 and 74. Substitution happens in the release-time `genesis-ceremony`
workflow, which commits the generated file onto the *release* branch, so `master`
never receives it. libp2p matches protocol names by exact string equality
(`consensus/cryptarchia-sync/src/libp2p/behaviour.rs:161`), so the mismatch is
total.

The same line at tag `0.2.0` reads
`/logos-blockchain-testnet-0.2.0/chainsync/1.0.0`, and `git diff 0.2.0
origin/testnet` on that file is empty — **the deployed fleet's protocol identity
is exactly tag `0.2.0`.**

**Action: build `logos-blockchain-module` at tag `0.2.0`**
(`88d57c92b51abea1ee737144ec3d08df69594794`, which pins
`logos-blockchain?ref=0.2.0`). That produces `blockchain_module-0.2.0.lgx` —
the artifact the operator guide names by filename. Two shortcuts that do **not**
work, both checked:

- *Patching the protocol string on a master build.* `master` and `0.2.0` carry
  different genesis `block_root`s (`c3dae14b…` vs `0f9b2854…`). They are
  different chains, not different labels.
- *Feeding testnet's deployment file to a master build via the real
  `custom_deployment_path` hatch* (`c-bindings/src/api/lifecycle.rs:37`, the
  second argument to `start()`). It parses with `OnUnknownKeys::Fail` and
  `0.2.0`'s file carries keys `master` no longer knows — and that schema drift
  is itself evidence the wire format moved.

Reported upstream as a packaging defect: a build from the default branch should
fail loudly rather than start and blame its peers.

The bootstrap peers, verbatim from the operator guide (Testnet 0.2), are
supplied as `initial_peers` in the `generate_user_config` JSON:

```
/ip4/65.109.51.37/udp/3000/quic-v1/p2p/12D3KooWFrouXfmrR4nsLMtE7wu15DoMJ6VtoUtHinREZCvbWHar
/ip4/65.109.51.37/udp/3001/quic-v1/p2p/12D3KooWJRGau8M1rjT7R5e4YYsgdFhsMX35nRDtMwCDjxQkXAHz
/ip4/65.109.51.37/udp/3002/quic-v1/p2p/12D3KooWQXJavMDTRscjauFSgVAB1VLB6Rzpy2uY5SU9Tk7927tb
/ip4/65.109.51.37/udp/50001/quic-v1/p2p/12D3KooWSQc7CcGtvWDPF1yCbBthFnQjprfCVHmfmNDUrSmqQsU1
```

These same four peers work unchanged on tag `0.2.0` (§2b) — they were never the
problem, so no peer list needs revisiting.

On `master` the symptom is `mode: NotStarted` with `height: 0` until the abort.
That failure mode is still worth keeping in view for the UI: it is exactly what a
version-skewed or offline node looks like, and the monitor should render it
legibly rather than as a blank pane. But it is no longer blocking — block-driven
features can now be verified end to end against a syncing `0.2.0` node.

## 6b. What has actually been exercised

Everything below was run end to end against a real daemon, so it is the set of
behaviours the code above may rely on.

| | outcome |
|---|---|
| build | done, twice — `master` and tag `0.2.0`. Tag `0.2.0` is what we keep |
| binary checks | pass — no circuits references; Qt 6.9.2 matching our live plugins |
| manifest | pass — `blockchain_module` / `0.2.0` / sole key `darwin-arm64-dev` |
| stage | staged together: chat 0.2.1 (patched), delivery 0.1.3, capability 1.0.0, blockchain 0.2.0 |
| pinning | the built artifact is pinned so a store collection cannot remove it |
| chat unaffected | pass, and under load (§7) |
| load | pass |
| contract | **differs from the prediction** — 24 methods / 1 event, not 29/3 (§2) |
| `generate_user_config` | pass — returns a path under the instance dir; **once-only** (§3) |
| `start` | pass — and is asynchronous; second call semantics not re-tested at `0.2.0` |
| `get_cryptarchia_info` | pass — `mode` reported, chain syncing (§2b). `get_time_info` **absent** |
| log routing | pass — two envelope forms, three sublanguages, fixture captured (§4) |
| events | pass — 1158 `newBlock` signals; note the CLI `watch` does not show them (§2b) |

Two expectations that looked safe turned out wrong, and the code depends on
the corrections: the *shippable* build has the 24/1 surface rather than 29/3,
and `get_time_info` is not available to drive the status strip's denominator
(§2 gives the client-side substitute).

## 7. Regression check

Staging blockchain into the shared modules directory is a **no-op for chat**,
verified with both blockchain builds staged in turn: `load-module
chat_module` auto-loaded `delivery_module` as its declared dependency,
`init {"delivery_preset": "logos.test"}` succeeded, and `get_address` returned a
real 32-byte address, with all four modules reporting `loaded` simultaneously.

The v6 run is the stronger check, because chat was brought up **while the
blockchain node was actively syncing** — competing for CPU, disk and network in
the same daemon — and neither disturbed the other.

That run also demonstrates the node reaching the chain head. During catch-up
height climbed ~15,000 per 30s; once caught up it advanced by 7 over the next 30s
while `slot` continued to increment about once per second. That difference —
height tracking real block production rather than bulk download — is the
observable signature of a synced node, and is available **~20 minutes before**
`mode` flips to `Online` (§2b). So height-versus-slot is the better "we are
current" signal for the UI; `mode` is the slower, more conservative one. Show
both — they answer different questions.
