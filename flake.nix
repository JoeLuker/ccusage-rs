{
  description = "Claude Code usage & cost reports from local JSONL transcripts";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      meta.status = "active";

      packages = forAll (pkgs: rec {
        ccusage-rs = pkgs.rustPlatform.buildRustPackage {
          pname = "ccusage-rs";
          version = "0.1.0";
          # This CLI has no embedded assets or external test fixtures.
          # Documentation and harness changes do not affect its Rust build.
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ];
          };
          cargoLock.lockFile = ./Cargo.lock;
        };
        default = ccusage-rs;
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ rustc cargo rust-analyzer clippy rustfmt ];
        };
      });
    };
}
