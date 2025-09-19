{
  description = "Wassette - A security-oriented runtime that runs WebAssembly Components via MCP";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, rust-overlay, flake-utils, crane, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
          targets = [ "wasm32-wasip2" "wasm32-wasip1" "wasm32-unknown-unknown" ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        commonArgs = {
          src = pkgs.lib.cleanSourceWith {
            src = craneLib.path ./.;
            filter = path: type:
              (craneLib.filterCargoSources path type)
              || (pkgs.lib.hasSuffix "README.md" path)
              || (pkgs.lib.hasSuffix "component-registry.json" path);
          };
          strictDeps = true;

          buildInputs = with pkgs; [
            openssl
          ] ++ lib.optionals stdenv.isDarwin [
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
          ];

          nativeBuildInputs = with pkgs; [
            pkg-config
            rustToolchain
          ];

          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        wassette = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "wassette";
          version = (craneLib.crateNameFromCargoToml { inherit (commonArgs) src; }).version;
          doCheck = false; # Tests require building wasm components which needs additional setup
        });

      in {
        packages = {
          default = wassette;
          wassette = wassette;
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ wassette ];

          packages = with pkgs; [
            # Rust tooling
            rustToolchain
            cargo-watch
            cargo-edit
            cargo-expand
            cargo-nextest
            cargo-component

            # Wasm tools
            wasmtime
            wasm-tools
            wasm-pack

            # Build tools
            just
            pkg-config
            openssl

            # Development tools
            git
            curl
            jq

            # Language support for examples
            python3
            nodejs_22
            go
            uv
          ];

          shellHook = ''
            echo "🚀 Wassette development environment"
            echo ""
            echo "Available commands:"
            echo "  cargo build     - Build the project"
            echo "  cargo test      - Run tests"
            echo "  cargo run       - Run wassette"
            echo "  just            - List available just recipes"
            echo ""
            echo "Rust toolchain: $(rustc --version)"
            echo "Cargo component: $(cargo component --version 2>/dev/null || echo 'cargo-component is included in this dev shell (if this fails, check your environment)')"
            echo ""
          '';

          RUST_BACKTRACE = 1;
          RUST_LOG = "debug";
        };

        apps.default = flake-utils.lib.mkApp {
          drv = wassette;
        };
      }
    );
}