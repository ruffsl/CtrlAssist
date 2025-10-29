{
  description = "CtrlAssist Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in {
        devShells.default = pkgs.mkShell {
          buildInputs = [
            pkgs.cargo
            pkgs.clang
            pkgs.gcc
            pkgs.jstest-gtk
            pkgs.pkg-config
            pkgs.rustc
            pkgs.udev
          ];
          shellHook = ''
            echo "CtrlAssist Rust dev shell activated."
          '';
        };
      }
    );
}
