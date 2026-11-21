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
      self,
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
    {
      nixosModules.default = import ./packaging/nix/module.nix;

      # `pkgs.ctrlassist`, built against the consumer's nixpkgs, which is
      # what the module looks for first.
      overlays.default = final: _prev: {
        ctrlassist = final.callPackage ./packaging/nix/package.nix { };
      };
    }
    // eachSystem (
      system:
      let
        overlays = [ rust-overlay.overlays.default ];
        pkgs = import nixpkgs { inherit overlays system; };
        # The toolchain pin lives in rust-toolchain.toml, shared with rustup.
        rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        # llvm-cov and llvm-profdata for cargo-llvm-cov, whose LLVM major
        # must match the pinned rustc's.
        llvmPkgs = pkgs.llvmPackages_23;
        # tombi (TOML formatter, `just fmt`) pinned to the version CI
        # installs (`tombi@1.7.1` in ci.yml), so local and CI formatting
        # agree. Bump the two together.
        tombiSrc = pkgs.fetchFromGitHub {
          owner = "tombi-toml";
          repo = "tombi";
          tag = "v1.7.1";
          hash = "sha256-eeBtmMrkDWOADw7FMLLD8hYq5WYg/aFyATtYIMFYFk0=";
        };
        tombi = pkgs.tombi.overrideAttrs {
          version = "1.7.1";
          src = tombiSrc;
          cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
            src = tombiSrc;
            hash = "sha256-YJLhWIwB3Y8836Rq4ahRzBSnhp7bH8w2/oXfRVTOPXg=";
          };
        };
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

        # The pieces the virtual machine checks need, each one crate's
        # binaries: the test fixture, and the hiding service by itself.
        packages.fixture = pkgs.callPackage ./packaging/nix/crate.nix {
          crate = "ctrlassist-test";
          mainProgram = "ctrlassist-fixture";
        };
        packages.hider = pkgs.callPackage ./packaging/nix/crate.nix {
          crate = "ctrlassist-hider";
          mainProgram = "ctrlassist-hider";
          systemdUnits = [
            ./packaging/systemd/ctrlassist-hider.socket
            ./packaging/systemd/ctrlassist-hider.service
          ];
        };

        # CtrlAssist as it is installed, in the smaller profile.
        packages.default = pkgs.callPackage ./packaging/nix/package.nix {
          cargoProfile = "dist";
        };

        # The virtual machine checks; see docs/testing.md.
        checks.platform = pkgs.callPackage ./packaging/nix/test-platform.nix {
          fixture = self.packages.${system}.fixture;
        };
        checks.device = pkgs.callPackage ./packaging/nix/test-device.nix {
          fixture = self.packages.${system}.fixture;
        };
        checks.hider = pkgs.callPackage ./packaging/nix/test-hider.nix {
          inherit (self.packages.${system}) fixture hider;
        };
        checks.daemon = pkgs.callPackage ./packaging/nix/test-daemon.nix {
          inherit (self.packages.${system}) fixture;
          package = self.packages.${system}.default;
          module = self.nixosModules.default;
        };

        # Every tool a `just` recipe or prek hook calls comes from here, so
        # the checks run the same for everyone. A tool joins this list in
        # the commit that first uses it.
        devShells.default =
          let
            # cargo-nfpm reads package metadata from Cargo.toml and calls
            # nfpm for `just package-*`; its source carries its own Nix
            # package. TisVlay's pin.
            cargoNfpmSrc = pkgs.fetchCrate {
              pname = "cargo-nfpm";
              version = "0.1.3";
              hash = "sha256-PpflbxUL6/xwqeHw2OhPLcB1fi0VLbItzhdYkLqZuP0=";
            };
            cargoNfpm = pkgs.callPackage (import "${cargoNfpmSrc}/nix/package.nix") { };
          in
          pkgs.mkShell {
          packages = with pkgs; [
            actionlint
            # fetches the controller database snapshot (`just update-controller-db`)
            curl
            # nixpkgs' cargo-llvm-cov fails its own test suite in the sandbox.
            cargo-deny
            (pkgs.cargo-llvm-cov.overrideAttrs { doCheck = false; })
            cargo-nextest
            cargo-shear
            # The distro packages (`just package-*`). cargo-auditable embeds
            # each binary's dependency list. cargo-zigbuild and zig drive
            # the build: zig is the linker, so the binary carries an
            # explicit oldest glibc and no Nix store path as its
            # interpreter, as a plain build from this shell would. nfpm is
            # the packager cargo-nfpm calls.
            cargo-auditable
            # Writes the third-party license bundle (`just licenses-html`).
            cargo-about
            cargoNfpm
            cargo-zigbuild
            zig
            nfpm
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
            # sdl2-jstest: what an SDL game lists, for hardware checks of hiding
            sdl-jstest
            # TOML formatter (`just fmt`; config in .config/tombi.toml)
            tombi
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
