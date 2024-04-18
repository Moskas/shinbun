{
  description = "Rust dev flake";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    naersk.url = "github:nix-community/naersk";
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs =
    {
      self,
      flake-utils,
      naersk,
      nixpkgs,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = (import nixpkgs) { inherit system; };
        naersk' = pkgs.callPackage naersk { };
      in
      {
        defaultPackage = naersk'.buildPackage {
          src = ./.;
          nativeBuildInputs = with pkgs; [
            pkg-config
            rustc
          ];
          buildInputs = with pkgs; [ openssl ];
        };

        packages = {
          check = naersk'.buildPackage {
            src = ./.;
            nativeBuildInputs = with pkgs; [
              pkg-config
              rustc
            ];
            buildInputs = with pkgs; [ openssl ];
            mode = "check";
          };
        };

        devShell = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustc
            cargo
            cargo-watch
            rust-analyzer
            rustPackages.clippy
            rustfmt
            openssl
            pkg-config
          ];
        };
      }
    );
}
