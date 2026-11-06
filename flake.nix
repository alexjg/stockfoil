{
  description = "stockfoil";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-26.05";
    nixos-unstable.url = "nixpkgs/nixos-unstable-small";

    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    wasm-bodge-src = {
      url = "github:alexjg/wasm-bodge/v0.5.0";
      flake = false;
    };
  };

  outputs = {
    self,
    nixpkgs,
    nixos-unstable,
    flake-utils,
    rust-overlay,
    wasm-bodge-src,
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        overlays = [
          (import rust-overlay)
        ];

        pkgs = import nixpkgs {
          inherit system overlays;
        };

        unstable = import nixos-unstable {
          inherit system overlays;
        };

        nodejs = pkgs.nodejs_24;
        rustVersion = "1.91.0";

        rust-toolchain = pkgs.rust-bin.stable.${rustVersion}.default.override {
          extensions = [
            "cargo"
            "clippy"
            "llvm-tools-preview"
            "rust-src"
            "rust-std"
            "rustfmt"
          ];
          targets = [
            "wasm32-unknown-unknown"
          ];
        };

        # wasm-bodge's default panic=unwind build uses `cargo +nightly` with
        # `-Zbuild-std=std,panic_unwind`, which requires a nightly toolchain
        # with `rust-src` and the `wasm32-unknown-unknown` target.
        wasm-rust-toolchain = pkgs.rust-bin.nightly.latest.minimal.override {
          extensions = [ "rust-src" ];
          targets = [ "wasm32-unknown-unknown" ];
        };

        # Translate `cargo +nightly ...` invocations from wasm-bodge to the
        # Nix-provided nightly toolchain without requiring rustup.
        cargo-with-nightly = pkgs.writeShellScriptBin "cargo" ''
          if [ "''${1:-}" = "+nightly" ]; then
            shift
            export PATH="${wasm-rust-toolchain}/bin:$PATH"
            exec "${wasm-rust-toolchain}/bin/cargo" "$@"
          fi
          exec "${rust-toolchain}/bin/cargo" "$@"
        '';

        # Must match the `wasm-bindgen = "=0.2.127"` version in javascript/Cargo.toml.
        wasm-bindgen-cli = unstable.buildWasmBindgenCli rec {
          src = pkgs.fetchCrate {
            pname = "wasm-bindgen-cli";
            version = "0.2.127";
            hash = "sha256-di+qBAdd7pENLiIB9CoZoab+W5xeDoByMREcCGTSzWo=";
          };

          cargoDeps = unstable.rustPlatform.fetchCargoVendor {
            inherit src;
            inherit (src) pname version;
            hash = "sha256-FTv2GZIAQs0ePdIZXIXil7JbZ6kIT05VG6vqC1qNFxQ=";
          };
        };

        wasm-bodge-rustPlatform = pkgs.makeRustPlatform {
          cargo = rust-toolchain;
          rustc = rust-toolchain;
        };

        wasm-bodge = wasm-bodge-rustPlatform.buildRustPackage {
          pname = "wasm-bodge";
          version = "0.5.0";
          src = wasm-bodge-src;
          cargoHash = "sha256-KE/AAkrdQ/tmr1X4Fya9CU/oH8e166qJax2kZ3R6jX0=";
          nativeBuildInputs = [ unstable.cargo-auditable ];
          doCheck = false;
        };

        native-ci-inputs = [ pkgs.stdenv.cc ];

        wasm-ci-inputs =
          [
            (pkgs.lib.hiPrio cargo-with-nightly)
            rust-toolchain
            wasm-rust-toolchain
            wasm-bodge
            wasm-bindgen-cli
            pkgs.binaryen
            nodejs
            pkgs.esbuild
            pkgs.typescript
          ]
          ++ native-ci-inputs;

        # Keep CI entry points in the flake so local development and GitHub
        # Actions invoke the same scripts with the same pinned tools.
        mk-ci-command = name: runtimeInputs: text:
          pkgs.writeShellApplication {
            inherit name;
            runtimeInputs = [ pkgs.git ] ++ runtimeInputs;
            text = ''
              repo_root="$(${pkgs.git}/bin/git rev-parse --show-toplevel)"
              cd "$repo_root"
              ${pkgs.lib.optionalString pkgs.stdenv.isDarwin ''
                export NIX_LDFLAGS="-L${pkgs.libiconv}/lib ''${NIX_LDFLAGS:-}"
                export LIBRARY_PATH="${pkgs.libiconv}/lib''${LIBRARY_PATH:+:$LIBRARY_PATH}"
              ''}
              ${text}
            '';
          };

        ci-fmt = mk-ci-command "ci-fmt" [ rust-toolchain ] ''
          exec ./scripts/ci/fmt
        '';

        ci-lint = mk-ci-command "ci-lint" ([ rust-toolchain pkgs.findutils ] ++ native-ci-inputs) ''
          exec ./scripts/ci/lint
        '';

        ci-docs = mk-ci-command "ci-docs" ([ rust-toolchain ] ++ native-ci-inputs) ''
          exec ./scripts/ci/docs
        '';

        ci-test-rust = mk-ci-command "ci-test-rust" ([ rust-toolchain ] ++ native-ci-inputs) ''
          exec ./scripts/ci/test-rust
        '';

        ci-test-fuzz = mk-ci-command "ci-test-fuzz" ([ rust-toolchain ] ++ native-ci-inputs) ''
          exec ./scripts/ci/test-fuzz
        '';

        ci-test-js = mk-ci-command "ci-test-js" wasm-ci-inputs ''
          exec ./scripts/ci/test-js
        '';

        ci = mk-ci-command "ci" [
          ci-fmt
          ci-lint
          ci-docs
          ci-test-rust
          ci-test-fuzz
          ci-test-js
        ] ''
          ci-fmt
          ci-lint
          ci-docs
          ci-test-rust
          ci-test-fuzz
          ci-test-js
        '';

        ci-packages = {
          inherit
            ci
            ci-fmt
            ci-lint
            ci-docs
            ci-test-rust
            ci-test-fuzz
            ci-test-js
            ;
        };
      in {
        devShells.default = pkgs.mkShell {
          name = "stockfoil";

          nativeBuildInputs = [
            (pkgs.lib.hiPrio cargo-with-nightly)
            rust-toolchain
            wasm-rust-toolchain
            wasm-bodge
            wasm-bindgen-cli
            pkgs.binaryen
            nodejs
            pkgs.esbuild
            pkgs.typescript
            pkgs.rust-analyzer
            pkgs.alejandra
          ];
        };

        packages = ci-packages;

        apps =
          builtins.mapAttrs (name: package: {
            type = "app";
            program = "${package}/bin/${name}";
            meta.description = "Run the ${name} repository command";
          })
          ci-packages;

        formatter = pkgs.alejandra;
      }
    );
}
