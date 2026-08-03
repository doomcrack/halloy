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
