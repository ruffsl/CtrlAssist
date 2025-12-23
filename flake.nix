{
  description = "CtrlAssist Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ rust-overlay.overlays.default ];
        pkgs = import nixpkgs { inherit overlays system; };
        rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      in
      {
        devShell = pkgs.mkShell {
          packages = with pkgs; [
            appstream
            # clang
            flatpak-builder
            # gcc
            jstest-gtk
            linuxConsoleTools
            lldb
            pkg-config
            python313Packages.aiohttp
            python313Packages.tomlkit
            rust
            udev
          ];
        };
      }
    );
}
