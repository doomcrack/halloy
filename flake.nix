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
    logos-protocol.url = "github:logos-co/logos-protocol";
    nixpkgs.follows = "logos-protocol/nixpkgs";

    logoscore.url = "github:logos-co/logos-logoscore-cli";
    # Keep the daemon and the dylib logos-sys links against on the same
    # protocol rev (same follows-wiring logoscore uses for its own SDK stack).
    logoscore.inputs.logos-protocol.follows = "logos-protocol";
  };

  outputs = { self, nixpkgs, logos-protocol, logoscore }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f {
        pkgs = import nixpkgs { inherit system; };
        protocolLib = logos-protocol.packages.${system}.logos-protocol-lib;
        logoscoreCli = logoscore.packages.${system}.cli;
      });
    in
    {
      devShells = forAllSystems ({ pkgs, protocolLib, logoscoreCli }: {
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
