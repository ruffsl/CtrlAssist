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

        # slint-lsp and slint-viewer at the workspace's Slint version,
        # which nixpkgs' may trail.
        packages.slint-lsp = pkgs.callPackage ./packaging/nix/slint-lsp.nix { };
        packages.slint-viewer = pkgs.callPackage ./packaging/nix/slint-viewer.nix { };

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
            # What the settings window loads at run time, which a build
            # doesn't link; the dev shell puts it on LD_LIBRARY_PATH so
            # `cargo run` and the tests find it.
            slintRuntimeLibs = pkgs.callPackage ./packaging/nix/slint-runtime.nix { };
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
            # The pinned one, which keeps the Slint crates' license text.
            (pkgs.callPackage ./packaging/nix/cargo-about.nix { })
            cargoNfpm
            cargo-zigbuild
            zig
            nfpm
            # The release's supply-chain checks. cargo-cyclonedx writes one
            # SBOM per shipped binary (`just sbom`), which `just sbom-check`
            # compares with what cargo-auditable embedded, read back by
            # rust-audit-info; jq and uuidgen (util-linux) shape them.
            # CycloneDX re-published the 0.5.10 tag after nixpkgs hashed it;
            # the trees differ only in upstream's own release tooling, so
            # the tag's current hash is taken. Drop the override once
            # nixpkgs carries it. TisVlay's workaround.
            (
              let
                src = pkgs.fetchFromGitHub {
                  owner = "CycloneDX";
                  repo = "cyclonedx-rust-cargo";
                  rev = "cargo-cyclonedx-0.5.10";
                  hash = "sha256-Ne+oufpXfI19KwRjnBAPeSM65ja+5O26+Yl8e+DUsjM=";
                };
              in
              pkgs.cargo-cyclonedx.overrideAttrs {
                inherit src;
                cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
                  inherit src;
                  hash = "sha256-aUfygtpGleCpuetyJHGHzs8J5VFkrk996ZqmTYgTwEk=";
                };
              }
            )
            rust-audit-info
            jq
            util-linux
            # `just verify-assets` and `just verify-release`: gh checks the
            # attestations, bsdtar (libarchive) unpacks all three package
            # formats, and cosign runs the check for people without a
            # GitHub account.
            gh
            libarchive
            cosign
            # `just release-bump`: the version and the CHANGELOGs from the
            # commit history.
            release-plz
            # The user guide (`just docs-*`): mdBook builds it, and lychee
            # checks its local links and anchors.
            mdbook
            lychee
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
            # The settings window's UI files: slint-lsp for the editor's
            # completion and live preview, slint-viewer to check and
            # render them without the app, both at the workspace's Slint
            # version.
            self.packages.${system}.slint-lsp
            self.packages.${system}.slint-viewer
            # TOML formatter (`just fmt`; config in .config/tombi.toml)
            tombi
            typos
            udev
            zizmor
          ]
          ++ slintRuntimeLibs;

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath slintRuntimeLibs;

          LLVM_COV = "${llvmPkgs.bintools-unwrapped}/bin/llvm-cov";
          LLVM_PROFDATA = "${llvmPkgs.bintools-unwrapped}/bin/llvm-profdata";

          RUST_SRC_PATH = "${rust}/lib/rustlib/src/rust/library";
        };
      }
    );
}
