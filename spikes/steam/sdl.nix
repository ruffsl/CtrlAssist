# What does SDL3 make of a Steam Controller (2026) made through uhid,
# under the descriptor written here and under the retail one? For the
# identity's choice of descriptor.
#
#   nix build --impure --expr 'import ./spikes/steam/sdl.nix { }' -L
{
  pkgs ?
    let
      locked = (builtins.fromJSON (builtins.readFile ../../flake.lock)).nodes.nixpkgs.locked;
    in
    import (builtins.fetchTree locked) { },
}:
let
  spike = pkgs.rustPlatform.buildRustPackage {
    pname = "steam-spike";
    version = "0.0.0";
    src = pkgs.lib.fileset.toSource {
      root = ./.;
      fileset = pkgs.lib.fileset.unions [
        ./Cargo.toml
        ./Cargo.lock
        ./src
      ];
    };
    cargoLock.lockFile = ./Cargo.lock;
  };
  probe = pkgs.runCommandCC "sdl-probe" { buildInputs = [ pkgs.sdl3 ]; nativeBuildInputs = [ pkgs.pkg-config ]; } ''
    mkdir -p $out/bin
    $CC -O1 -o $out/bin/sdl-probe ${./probe.c} $(pkg-config --cflags --libs sdl3)
  '';
in
pkgs.testers.runNixOSTest {
  name = "ctrlassist-spike-steam-sdl";
  nodes.machine = {
    boot.kernelPackages = pkgs.linuxPackages_7_3;
    boot.kernelModules = [
      "uhid"
      "hid-steam"
    ];
    environment.systemPackages = [
      spike
      probe
    ];
  };
  testScript = builtins.readFile ./sdl.py;
  skipTypeCheck = true;
}
