{
  description = "Frigicom - a Logos chat client (halloy fork)";

  # Pre-built Logos artifacts (delivery module, liblogos, …) come from the
  # self-hosted Logos Attic cache, for local builds too — mirrors the
  # nixConfig in logos-chat-module's flake so `.#install` and the logoscore
  # stack resolve without building the world. Read-only and public.
  nixConfig = {
    extra-substituters = [ "https://cache.nix.logos.co/public" ];
    extra-trusted-public-keys = [ "public:l4HrXgL4nw246+LBh2SOJyhz64BoGegOYLheT/iIAPU=" ];
  };

  inputs = {
    # NOTE: this tracks a branch, and it is currently a documented skew —
    # the rev the lock holds builds `logos-protocol-lib-0.1.0` while the
    # dylib `dev-env.sh` actually uses is 0.2.0 from a GC root. So
    # `nix develop` and `dev-env.sh` are NOT equivalent today, despite
    # being documented as such. Closing it means relinking the app against
    # a different ABI, so it wants its own pass rather than riding a daemon
    # bump. See docs/dependency-maintenance.md.
    logos-protocol.url = "github:logos-co/logos-protocol";
    nixpkgs.follows = "logos-protocol/nixpkgs";

    # Tag 0.2.2, not the default branch. 0.2.2-RC1 carries the fix for a
    # platform bug where a cross-module `sign` returned a *different*
    # caller's argument once a second caller had called — which the chat
    # identity depends on not happening. Pinning a tag rather than a branch
    # also means the daemon stops moving under us between builds.
    logoscore.url = "github:logos-co/logos-logoscore-cli/0.2.2";
    # Keep the daemon and the dylib logos-sys links against on the same
    # protocol rev (same follows-wiring logoscore uses for its own SDK stack).
    logoscore.inputs.logos-protocol.follows = "logos-protocol";

    # The module set `packages.modules` stages. Pinned here rather than
    # merged by hand so the tree a release is built from is derivable from
    # a recipe — see the output below for why that matters.
    #
    # chat_module is OUR FORK. Upstream is logos-co/logos-chat-module; the
    # `frigicom` branch is the two fix branches merged, and it exists
    # because those two were previously being combined in a working tree
    # at build time — which shipped a chat module that was on no branch at
    # all. Pinning either half alone silently drops the other's fixes.
    # Anything built from this tree runs a chat module that exists nowhere
    # else, which is why the tree records its own provenance.
    chat-module.url = "github:doomcrack/logos-chat-module/frigicom";

    # Held in lockstep with what chat_module was compiled against: the chat
    # flake re-exports this exact input as `delivery_module-lgx` precisely
    # so the pair cannot drift.
    delivery-module.url = "github:logos-co/logos-delivery-module/v0.1.3";
    chat-module.inputs.logos-delivery-module.follows = "delivery-module";

    # Tag 0.2.0, not master. Master advertises an unsubstituted protocol
    # name and cannot find a peer; the deployed fleet's protocol identity
    # is exactly this tag. docs/logos-modules.md §6 has the evidence.
    blockchain-module.url = "github:logos-blockchain/logos-blockchain-module/0.2.0";

    # Holds the chat identity key and never exports it. chat_module declares
    # it as a dependency, so the daemon auto-loads it — but it still has to
    # be staged here, because `load-module` resolves declared dependencies
    # only from the directory it was pointed at.
    keystore-signer-module.url = "github:doomcrack/keystore-signer-module";
  };

  outputs =
    { self
    , nixpkgs
    , logos-protocol
    , logoscore
    , chat-module
    , delivery-module
    , blockchain-module
    , keystore-signer-module
    }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f {
        inherit system;
        pkgs = import nixpkgs { inherit system; };
        protocolLib = logos-protocol.packages.${system}.logos-protocol-lib;
        logoscoreCli = logoscore.packages.${system}.cli;
      });

      # What each module in the tree is and where it came from, recorded
      # from the locked inputs above rather than written down by hand. The
      # build folds each module's own manifest version in on top.
      sources = {
        chat_module = {
          flake = "github:doomcrack/logos-chat-module";
          rev = chat-module.rev;
          upstream = "github:logos-co/logos-chat-module";
          patched = true;
          # `rev` above is what actually determines the build; this list is
          # for a reader who wants to know what differs without cloning it,
          # and has to be updated whenever the pin moves.
          patches = [
            "libchat patched over git so an inbound duplicate Welcome is skipped rather than aborting the module (doomcrack/libchat, fix/idempotent-welcome)"
            "delivery is sent only the config keys every build accepts"
            "delivery is reported online only once the node really started"
            "delivery replies that reject a send or a subscribe are logged"
          ];
        };
        delivery_module = {
          flake = "github:logos-co/logos-delivery-module";
          rev = delivery-module.rev;
          patched = false;
        };
        blockchain_module = {
          flake = "github:logos-blockchain/logos-blockchain-module";
          rev = blockchain-module.rev;
          patched = false;
        };
        capability_module = {
          flake = "github:logos-co/logos-logoscore-cli";
          rev = logoscore.rev;
          patched = false;
        };
        keystore_signer = {
          flake = "github:doomcrack/keystore-signer-module";
          rev = keystore-signer-module.rev;
          patched = false;
        };
      };
    in
    {
      # The merged module tree, which no upstream output provides: the
      # daemon only reaches the network with chat, delivery and capability
      # in ONE directory, and each of them ships from a different place and
      # in a different shape. Staging it by hand is what this replaces —
      # a hand-merged tree lives on exactly one machine, and a disk image
      # built from one cannot be reproduced or audited by anybody, us
      # included.
      #
      #   nix build .#modules --out-link result-modules
      #   . scripts/dev-env.sh      # prefers result-modules/modules over
      #                             # anything staged by hand
      packages = forAllSystems ({ system, pkgs, logoscoreCli, ... }:
        let
          # The blockchain module cannot be built for Intel macOS: its
          # `logos-blockchain-circuits` dependency publishes no
          # `x86_64-darwin` output at all, so the whole tree fails to
          # *evaluate* there rather than failing to build.
          #
          # Staged conditionally so the other four still produce a usable
          # tree on that platform. Chat does not depend on blockchain, so
          # what is lost is the blockchain panel, not the client.
          blockchainSupported = system != "x86_64-darwin";
          stagedSources =
            if blockchainSupported
            then sources
            else builtins.removeAttrs sources [ "blockchain_module" ];
        in
        {
        modules = pkgs.runCommand "frigicom-modules"
          {
            nativeBuildInputs = [ pkgs.jq ];
            passAsFile = [ "provenance" ];
            provenance = builtins.toJSON stagedSources;
          } ''
          mkdir -p "$out/modules" "$out/bin"

          # The daemon ships with the modules it was built against.
          #
          # Not a convenience. Before this, `dev-env.sh` took the daemon
          # from a hand-made GC root while the modules came from the flake,
          # so bumping the logoscore pin moved `capability_module` and left
          # the daemon behind — the two silently disagreeing, which is the
          # hardest kind of skew to notice because everything still starts.
          # One output, one version.
          cp -RL ${logoscoreCli}/bin/. "$out/bin/"
          chmod -R u+w "$out/bin"

          # chat and capability ship already staged...
          cp -R ${chat-module.packages.${system}.install}/modules/chat_module \
            "$out/modules/"
          cp -R ${logoscoreCli}/modules/capability_module "$out/modules/"
          cp -R ${keystore-signer-module.packages.${system}.install}/modules/keystore_signer \
            "$out/modules/"

          # ...while delivery and blockchain ship as `.lgx` archives: a
          # gzipped tar holding a manifest and one directory per platform
          # variant. The loader wants that flattened, with the variant it
          # resolved recorded beside the manifest.
          stage_lgx() {
            local work name variant
            work=$(mktemp -d)
            tar -xzf "$1" -C "$work"

            name=$(jq -r .name "$work/manifest.json")
            variant=$(ls "$work/variants" | head -1)

            mkdir -p "$out/modules/$name"
            cp "$work/manifest.json" "$out/modules/$name/"
            cp -R "$work/variants/$variant/." "$out/modules/$name/"
            printf '%s' "$variant" > "$out/modules/$name/variant"
          }

          stage_lgx ${delivery-module.packages.${system}.lgx}/*.lgx
          ${if blockchainSupported
            then "stage_lgx ${blockchain-module.packages.${system}.lgx}/*.lgx"
            else "# blockchain_module: no x86_64-darwin build exists"}

          chmod -R u+w "$out"

          # The licence texts travel with the binaries they cover. Most of
          # these are upstream MIT/Apache-2.0 projects and one is our fork
          # of one; an image that ships the code without the terms is not
          # something we can hand to anybody.
          #
          # `keystore-signer-module` declares `MIT OR Apache-2.0` in its
          # manifest but ships no licence *text* at its root. A missing
          # file is recorded rather than silently skipped: shipping a
          # binary whose terms we cannot reproduce is a thing a reader
          # should be able to see, and it is worth asking upstream for.
          # A bash array, not a backslash-continued `for ... in` list: the
          # blockchain entry is optional (absent on x86_64-darwin), and an
          # empty interpolation between two backslash-continued lines leaves a
          # dangling continuation that terminates the list early. An empty
          # line inside an array literal is simply ignored.
          pairs=(
            "chat_module:${chat-module}"
            "delivery_module:${delivery-module}"
            ${if blockchainSupported
              then "\"blockchain_module:${blockchain-module}\""
              else ""}
            "capability_module:${logoscore}"
            "keystore_signer:${keystore-signer-module}"
          )
          for pair in "''${pairs[@]}"; do
            name=''${pair%%:*}
            src=''${pair#*:}
            mkdir -p "$out/licenses/$name"

            # `find`, not a glob: an unmatched glob inside this builder
            # expands to the literal pattern and `cp` then fails on a
            # path that does not exist.
            texts=$(find "$src" -maxdepth 1 -name 'LICENSE*' 2>/dev/null)

            if [ -n "$texts" ]; then
              printf '%s\n' "$texts" | while read -r text; do
                cp "$text" "$out/licenses/$name/"
              done
            else
              printf '%s\n' \
                'This component declares its licence in its package' \
                'manifest but ships no licence text in its source tree,' \
                'so none could be copied here. See its repository.' \
                > "$out/licenses/$name/NO-LICENCE-TEXT"
            fi
          done

          # Versions come off the manifests on disk rather than being
          # written down a second time, so the record cannot drift from
          # what was actually staged.
          for module in "$out"/modules/*; do
            jq -n --arg name "$(basename "$module")" \
                  --arg version "$(jq -r .version "$module/manifest.json")" \
                  '{($name): $version}'
          done | jq -s add > versions.json

          jq --slurpfile found versions.json '{
              modules: (to_entries
                | map(.value += { version: ($found[0][.key] // "unknown") })
                | from_entries)
            }' "$provenancePath" > "$out/provenance.json"
        '';
      });

      devShells = forAllSystems ({ pkgs, protocolLib, logoscoreCli, ... }: {
        default = pkgs.mkShell {
          name = "frigicom-dev";

          # The rust toolchain is intentionally NOT provided here: halloy
          # tracks "stable minus 3 releases" while this nixpkgs pin ships
          # rustc 1.89 (too old). rustup provides the toolchain; outside
          # `nix develop`, `scripts/dev-env.sh` exports the same variables
          # from GC-rooted store paths.
          shellHook = ''
            export LOGOS_PROTOCOL_ROOT="${protocolLib}"
            export LOGOSCORE_BIN="${logoscoreCli}/bin/logoscore"
            case "$(uname -s)" in
              Darwin) export DYLD_LIBRARY_PATH="''${DYLD_LIBRARY_PATH:+$DYLD_LIBRARY_PATH:}${protocolLib}/lib" ;;
              *) export LD_LIBRARY_PATH="''${LD_LIBRARY_PATH:+$LD_LIBRARY_PATH:}${protocolLib}/lib" ;;
            esac

            # chat.init only reaches the network with chat_module,
            # delivery_module and capability_module in ONE directory, and no
            # upstream output stages them together: logos-chat-module's
            # `.#install` gives chat_module, logoscore-cli gives
            # capability_module, and delivery ships as an .lgx archive. There
            # is therefore nothing to point LOGOS_MODULES_DIR at from here, so
            # defer to scripts/dev-env.sh, which picks the newest staged tree
            # out of $FRIGICOM_ARTIFACTS (default: <repo>/../.gcroots). It
            # keeps the two variables above, so only the module dir and the
            # linker path come from it. Stage a tree with, from a sibling
            # checkout, `nix build path:../logos-chat-module#install`, or just
            # export LOGOS_MODULES_DIR yourself.
            _frigicom_dev_env="$PWD/scripts/dev-env.sh"
            if [ ! -f "$_frigicom_dev_env" ]; then
              _frigicom_dev_env="$(git rev-parse --show-toplevel 2>/dev/null)/scripts/dev-env.sh"
            fi
            if [ -f "$_frigicom_dev_env" ]; then
              . "$_frigicom_dev_env"
            else
              echo "frigicom dev shell: scripts/dev-env.sh not found from $PWD; \
            export LOGOS_MODULES_DIR (a merged module tree) yourself"
            fi
            unset _frigicom_dev_env

            echo "frigicom dev shell: LOGOS_PROTOCOL_ROOT, LOGOSCORE_BIN, LOGOS_MODULES_DIR set"
            for _m in chat_module delivery_module capability_module; do
              [ -d "''${LOGOS_MODULES_DIR:-}/$_m" ] || \
                echo "  WARNING: $_m missing from ''${LOGOS_MODULES_DIR:-<unset>} — \
            the live backend will not come up; export LOGOS_MODULES_DIR to a merged tree"
            done
            unset _m
          '';
        };
      });
    };
}
