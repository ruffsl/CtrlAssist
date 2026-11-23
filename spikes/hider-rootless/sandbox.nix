# Which of systemd's file-system protections take effect in a unit with no
# capabilities, and which a write to /run and /sys gets past: a machine of
# its own, so the question can be asked without the fake pads.
#
#   nix build --impure --expr 'import ./spikes/hider-rootless/sandbox.nix { }' -L
{
  pkgs ?
    let
      locked = (builtins.fromJSON (builtins.readFile ../../flake.lock)).nodes.nixpkgs.locked;
    in
    import (builtins.fetchTree locked) { },
}:
pkgs.testers.runNixOSTest {
  name = "ctrlassist-spike-hider-sandbox";
  nodes.machine = {
    boot.kernelModules = [ "hid-playstation" ];
    systemd.tmpfiles.rules = [ "d /run/ctrlassist/hide 0755 root root -" ];
  };
  skipTypeCheck = true;
  testScript = builtins.readFile ./sandbox.py;
}
