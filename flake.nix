{
  description = "CtrlAssist - controller assist for gaming on Linux";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      # x86_64 and aarch64 Linux only: the tool is built on evdev and uinput.
      eachSystem =
        f:
        let
          bySystem = nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] f;
        in
        nixpkgs.lib.genAttrs [
          "checks"
          "devShells"
        ] (
          output: nixpkgs.lib.mapAttrs (_: outputs: outputs.${output}) bySystem
        );
    in
    eachSystem (
      system:
      let
        overlays = [ rust-overlay.overlays.default ];
        pkgs = import nixpkgs { inherit overlays system; };
        # The toolchain pin lives in rust-toolchain.toml, shared with rustup.
        rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      in
      {
        # The hiding spike's clean-machine probes, run in a NixOS VM.
        checks.spike-hiding = pkgs.callPackage ./spikes/hiding/vm.nix {
          probe = pkgs.rustPlatform.buildRustPackage {
            pname = "spike-hiding";
            version = "0.0.0";
            src = ./spikes/hiding;
            cargoLock.lockFile = ./spikes/hiding/Cargo.lock;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.udev ];
          };
        };

        # Every tool a `just` recipe or prek hook calls comes from here, so
        # the checks run the same for everyone. A tool joins this list in
        # the commit that first uses it.
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            cargo-nextest
            just
            just-lsp
            # jstest and evtest-style tools for looking at pads by hand
            linuxConsoleTools
            lldb
            pkg-config
            prek
            rust
            # sdl2-jstest: what an SDL game lists, for the hiding spike
            sdl-jstest
            typos
            udev
          ];

          RUST_SRC_PATH = "${rust}/lib/rustlib/src/rust/library";
        };
      }
    );
}
