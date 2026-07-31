# Privacy policy

Frigicom does not include telemetry and does not transfer information to other networked systems unless specifically requested by the user or the person installing or operating it.

Frigicom is a client for the Logos network. It starts a private `logoscore` daemon on the local machine and talks to it over loopback; the daemon's delivery module carries messages over the Logos network. Information required for that — your address, the addresses of the people and groups you chat with, message contents, and conversation membership — is transferred to the network by the delivery module.

Frigicom does not control the privacy practices of the Logos network or of the modules the daemon loads.

Two features reach outside the Logos network. Both are on by default and can be turned off in `config.toml`:

- **Link previews** (`[preview] enabled`) fetch a linked page directly from its host to render the preview card, which discloses your IP address to that host.
- **Check for update on launch** (`check_for_update_on_launch`) queries the GitHub releases API for the frigicom repository.
