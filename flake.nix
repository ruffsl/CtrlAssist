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
          "packages"
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
        # llvm-cov and llvm-profdata for cargo-llvm-cov, whose LLVM major
        # must match the pinned rustc's.
        llvmPkgs = pkgs.llvmPackages_23;
      in
      {
        # The oldest compiler the workspace supports, for `just msrv`: read
        # from Cargo.toml's `rust-version`, so the floor lives in one place.
        packages.msrv-rust =
          let
            floor = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.rust-version;
            parts = builtins.length (builtins.splitVersion floor);
          in
          pkgs.rust-bin.stable.${if parts == 2 then "${floor}.0" else floor}.minimal;

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
            actionlint
            # nixpkgs' cargo-llvm-cov fails its own test suite in the sandbox.
            (pkgs.cargo-llvm-cov.overrideAttrs { doCheck = false; })
            cargo-nextest
            just
            just-lsp
            # jstest and evtest-style tools for looking at pads by hand
            linuxConsoleTools
            lldb
            llvmPkgs.bintools-unwrapped
            pkg-config
            prek
            rust
            # actionlint runs it over the workflows' `run:` lines
            shellcheck
            # sdl2-jstest: what an SDL game lists, for the hiding spike
            sdl-jstest
            typos
            udev
            zizmor
          ];

          LLVM_COV = "${llvmPkgs.bintools-unwrapped}/bin/llvm-cov";
          LLVM_PROFDATA = "${llvmPkgs.bintools-unwrapped}/bin/llvm-profdata";

          RUST_SRC_PATH = "${rust}/lib/rustlib/src/rust/library";
        };
      }
    );
}
