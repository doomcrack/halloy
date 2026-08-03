# Live chat test — Intel ⇄ M3 over frigicom (logos.dev)

Both sides run a scriptable `logoscore` chat instance from the frigicom module
tree (`nbl.sh fork build .#modules` → `result-modules/modules`), init on
`delivery_preset = logos.dev`, and exchange messages over delivery/waku. Git is
only for trading addresses + coordinating who initiates.

## Addresses

- **Intel (x86_64):** `bdeaeb733fdbd43107fcd22c096a37f95469a3a9e71d56c5957764677cff7cb7`
  — instance up, `delivery_state: online`, waiting.
- **M3 (arm64):** _<fill in your `get_address` here>_

## Handshake (M3 does the create; Intel receives + replies)

M3 side:
```sh
# bring up a chat instance from your arm64 result-modules/modules, then:
CID=$($LOGOSCORE --config-dir "$CFG" call chat_module create_conversation \
  bdeaeb733fdbd43107fcd22c096a37f95469a3a9e71d56c5957764677cff7cb7 \
  | jq -r '.result')          # sends an MLS Welcome to the Intel address
$LOGOSCORE --config-dir "$CFG" call chat_module send_message "$CID" "hello from the M3, over frigicom"
```
Then commit this file with your address filled in + the CID, and push.

Intel side will: poll `get_messages`, see the conversation + your message, and
reply "hello back from Intel". Both then `get_messages` to confirm both
directions. Round-trip = chat works cross-arch over frigicom.

## Transcript (append as it happens)

- (Intel) instance online, address published, polling for an inbound conversation.

## Update (Intel) — initiator side blocks; please have M3 create

Intel tried `create_conversation` toward the M3 address several times: every call
returns `RPC_FAILED` and never reaches chat's Rust layer — the sync RPC blocks
(fetching the peer KeyPackage / publishing the Welcome) on a pubsub shard this
freshly-started node has **no mesh peers** on (`no mesh peer for /waku/2/rs/2/{1,3,4}`;
the RLN filter topic shows `no subscribed peers`). Repeated calls wedged the
daemon; restarted cleanly — same identity (keystore-persisted).

- **Intel address (stable):** `bdeaeb733fdbd43107fcd22c096a37f95469a3a9e71d56c5957764677cff7cb7`
- delivery_state: online; receiving relay traffic on shard 0.

**Please, M3:** try the create from your side instead —
`create_conversation bdeaeb73…7cb7` then `send_message`. Receiving + replying is
the path that worked in the docker run. If *your* create also `RPC_FAILED`s, then
it's a fleet-wide delivery/RLN mesh issue on logos.dev (not our build), and we
should compare notes (peer counts, whether your node meshes on shards 1/3/4,
maybe try `delivery_preset = logos.test` on both sides).

## Root cause found: wrong fleet + config in the wrong place

Both GUIs defaulted to **`logos.dev`**, whose relay mesh is starved on the chat
shards (thousands of `no mesh peer for /waku/2/rs/2/{1,3,4}`), so key-package /
Welcome exchange can't happen → `create_conversation` → `MethodFailed` (in the GUI
too, not just the CLI). On **`logos.test`** the same node gets `relayCount=11–13`
and healthy mesh.

The GUI reads its config from **`~/Library/Application Support/frigicom/config.toml`**
(NOT the repo `config.toml`, which is only a template). That file did not exist, so
it fell back to the `logos.dev` default.

### M3: switch to logos.test and retry
```sh
mkdir -p ~/Library/Application\ Support/frigicom
printf '[logos]\ndelivery_preset = "logos.test"\n' > ~/Library/Application\ Support/frigicom/config.toml
# then quit + relaunch frigicom (cargo run --features live, system toolchain)
```
Confirm the daemon log says `joining delivery preset logos.test`.

Addresses are keystore-derived, so they DON'T change with the fleet:
- **Intel GUI:** `e65a2214…` (now on logos.test, address `e65a2214`)
- **M3 GUI:** `0abbf2d3e82acf2a3ea2c9ee661b81e7f6a4e5f63e42f453f243147dafa4f1e8`

Once both are on logos.test, retry the GUI conversation (either side creates with
the other's address). This is the run that should finally connect.
