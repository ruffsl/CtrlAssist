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
            clang
            flatpak-builder
            # gcc
            jstest-gtk
            librsvg
            linuxConsoleTools
            lldb
            llvmPackages.libclang
            llvmPackages.llvm
            pkg-config
            python313Packages.aiohttp
            python313Packages.tomlkit
            rust
            udev
          ];
           # Use librsvg's gdk-pixbuf loader cache file as it enables gdk-pixbuf to load
           # SVG files (important for icons)
           # Fixes error: .../share/icons/hicolor/scalable/apps/blabla.svg is not a valid icon: Format not recognized
           GDK_PIXBUF_MODULE_FILE = "${pkgs.librsvg}/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache";
          LIBCLANG_PATH = pkgs.lib.makeLibraryPath [pkgs.llvmPackages.libclang.lib];
        };
      }
    );
}
