# Could the hiding service hide pads without writing udev rules at runtime,
# and without root? Four spikes on one machine; each result is a line
# starting FINDING.
#
#   nix build --impure --expr 'import ./spikes/hider-rootless/test.nix { }' -L
{
  # The nixpkgs the flake pins, read from its lock.
  pkgs ?
    let
      locked = (builtins.fromJSON (builtins.readFile ../../flake.lock)).nodes.nixpkgs.locked;
    in
    import (builtins.fetchTree locked) { },
  # The fake pads, as the flake builds them.
  fixture ? pkgs.callPackage ../../packaging/nix/crate.nix {
    crate = "ctrlassist-test";
    mainProgram = "ctrlassist-fixture";
  },
}:
let
  rules = pkgs.runCommand "ctrlassist-spike-rules" { } ''
    mkdir -p $out/lib/udev/rules.d
    for rule in ${./rules}/*.rules; do
      substitute $rule $out/lib/udev/rules.d/$(basename $rule) \
        --subst-var-by coreutils ${pkgs.coreutils} \
        --subst-var-by acl ${pkgs.acl}
    done
  '';
  fdpass = pkgs.writeShellScriptBin "fdpass" ''
    exec ${pkgs.python3}/bin/python3 ${./fdpass.py} "$@"
  '';
  hide = pkgs.writeShellScriptBin "spike-hide" (builtins.readFile ./hide.sh);
  xpad = pkgs.writeShellScriptBin "spike-xpad" ''
    exec ${pkgs.python3}/bin/python3 ${./xpad.py} "$@"
  '';
in
pkgs.testers.runNixOSTest {
  name = "ctrlassist-spike-hider-rootless";

  nodes.machine = {
    boot.kernelModules = [
      "uinput"
      "uhid"
      "hid-playstation"
      "joydev"
    ];

    users.groups.ctrlassist-hider = { };
    # The dedicated user a rootless service would run as.
    users.users.ctrlassist-hider = {
      isSystemUser = true;
      group = "ctrlassist-hider";
    };
    users.users.alice = {
      isNormalUser = true;
      uid = 1000;
      password = "alice";
      # A seat user who is also in the input group, as some distributions
      # and guides have gamers do.
      extraGroups = [ "input" ];
    };
    users.users.bob = {
      isNormalUser = true;
      uid = 1001;
      password = "bob";
    };

    # A graphical session for alice on the seat, as in the hider check.
    services.cage = {
      enable = true;
      user = "alice";
      program = "${pkgs.coreutils}/bin/sleep infinity";
    };
    virtualisation.qemu.options = [
      "-vga none -device virtio-gpu-pci"
      # A USB device with a serial number and a driver to re-bind on the
      # USB bus: a tablet, under usbhid.
      "-device qemu-xhci,id=xhci"
      "-device usb-tablet,bus=xhci.0,serial=CTRLSPIKE1"
    ];

    services.udev.packages = [ rules ];
    # The marker directory: the service's group may write it, and so may
    # root without capabilities, as its owner.
    systemd.tmpfiles.rules = [
      "d /run/ctrlassist 0755 root root -"
      "d /run/ctrlassist/hide 0775 root ctrlassist-hider -"
      "d /tmp/fdpass 1777 root root -"
    ];

    # What a cap-less root would start if it could reach the service manager.
    systemd.services.spike-escaped = {
      serviceConfig.Type = "oneshot";
      script = "touch /run/escaped";
    };

    environment.systemPackages = [
      fixture
      fdpass
      hide
      xpad
      pkgs.acl
      pkgs.kmod
      pkgs.usbutils
    ];
  };

  testScript = builtins.readFile ./test.py;
  # The script is a spike's: it reports more than it asserts.
  skipTypeCheck = true;
}
