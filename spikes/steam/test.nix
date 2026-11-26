# Does the kernel's hid-steam take a Steam Controller (2026) made through
# uhid, and what does it make of it? A stand-in on a 7.3 kernel, in a
# machine where it troubles nobody's desktop or Steam. Each observation
# is a line starting SPIKE, with a JSON object; the logs are copied out
# to the result.
#
#   nix build --impure --expr 'import ./spikes/steam/test.nix { }' -L
{
  # The nixpkgs the flake pins, read from its lock.
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
  inventory = pkgs.writeShellScriptBin "spike-inventory" ''
    exec ${pkgs.python3}/bin/python3 ${./inventory.py} ${pkgs.linuxHeaders}/include/linux "$@"
  '';
  # The hiding service's rule as packaged, its programs pointed at the
  # store, so that what it makes of the stand-in's nodes is seen.
  hideRule = pkgs.runCommand "ctrlassist-hide-rule" { } ''
    mkdir -p $out/lib/udev/rules.d
    substitute ${../../packaging/udev/72-ctrlassist-hide.rules} $out/lib/udev/rules.d/72-ctrlassist-hide.rules \
      --replace-quiet /usr/bin/udevadm ${pkgs.systemd}/bin/udevadm \
      --replace-quiet /usr/bin/ ${pkgs.coreutils}/bin/
  '';
in
pkgs.testers.runNixOSTest {
  name = "ctrlassist-spike-steam";

  nodes.machine = {
    # The first release with the 2026 controller in hid-steam, from the
    # binary cache.
    boot.kernelPackages = pkgs.linuxPackages_7_3;
    boot.kernelModules = [
      "uhid"
      "hid-steam"
    ];
    users.groups.ctrlassist-hider = { };
    users.users.ctrlassist-hider = {
      isSystemUser = true;
      group = "ctrlassist-hider";
    };
    services.udev.packages = [ hideRule ];
    systemd.tmpfiles.rules = [
      "d /run/ctrlassist 0755 root root -"
      "d /run/ctrlassist/hide 0775 root ctrlassist-hider -"
    ];
    environment.systemPackages = [
      spike
      inventory
      pkgs.evtest
    ];
  };

  testScript = builtins.readFile ./test.py;
  # The script is a spike's: it reports more than it asserts.
  skipTypeCheck = true;
}
