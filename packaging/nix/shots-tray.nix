# The tray's pictures for the user guide, made in a virtual machine: a
# Plasma session for one user, CtrlAssist as its NixOS module installs it,
# the test crate's fake pads, and a profile running on them. What the
# desktop draws of the tray is captured from the machine's screen, so the
# pictures are a real panel's and nobody's own.
#
# The result holds the pictures; `just docs-tray-shots` copies them into
# docs/screenshots.
{
  pkgs,
  fixture,
  package,
  module,
}:
pkgs.testers.runNixOSTest {
  name = "ctrlassist-tray-shots";

  nodes.machine = {
    imports = [ module ];
    services.ctrlassist = {
      enable = true;
      inherit package;
    };
    # The pads are the test crate's fake ones, which a daemon takes for
    # pads only when asked to.
    systemd.user.services.ctrlassist-daemon.environment.CTRLASSIST_TEST_PADS = "1";

    users.users.alice = {
      isNormalUser = true;
      uid = 1000;
      password = "alice";
    };

    # A Plasma session for alice, logged in as the machine starts.
    services.desktopManager.plasma6.enable = true;
    services.displayManager = {
      sddm.enable = true;
      sddm.wayland.enable = true;
      defaultSession = "plasma";
      autoLogin = {
        enable = true;
        user = "alice";
      };
    };

    virtualisation = {
      memorySize = 4096;
      cores = 4;
      resolution = {
        x = 1280;
        y = 800;
      };
      qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    };

    environment.systemPackages = [ fixture ];
  };

  testScript = ''
    import shlex

    ENV = "XDG_RUNTIME_DIR=/run/user/1000"

    def as_alice(command):
        return "su -l alice -c " + shlex.quote(ENV + " " + command)

    def cli(*arguments):
        return machine.succeed(as_alice("ctrlassist " + shlex.join(arguments)))

    machine.wait_for_unit("multi-user.target")
    # The session, its panel, and the tray in it.
    machine.wait_until_succeeds("pgrep -u alice plasmashell")
    machine.wait_until_succeeds(as_alice("systemctl --user is-active ctrlassist-tray"))
    machine.sleep(20)
    machine.screenshot("tray_idle")

    # Two fake pads, and a profile that runs on them.
    machine.succeed("systemd-run --unit=fake-assist ctrlassist-fixture fake-dualsense --real")
    machine.succeed(
        "systemd-run --unit=fake-primary ctrlassist-fixture fake-pad"
        " --name 'Xbox Wireless Controller' --vendor 045e --product 0b13"
    )
    machine.succeed("udevadm settle")
    machine.succeed(
        "printf '%s\\n' 'mode = \"priority\"' 'primary = \"model:045e:0b13\"'"
        " 'assist = \"02:ca:7e:00:00:01\"' 'identity = \"dualsense\"' > /tmp/couch.toml"
    )
    cli("profile", "put", "/tmp/couch.toml")
    machine.wait_until_succeeds(as_alice("ctrlassist start couch"))
    machine.sleep(3)
    machine.screenshot("tray_running")

    # The assist's pad leaves, and the setup is held for it.
    machine.succeed("systemctl stop fake-assist")
    machine.wait_until_succeeds(as_alice("ctrlassist setups --json") + " | grep -q '\"phase\":\"held\"'")
    machine.sleep(3)
    machine.screenshot("tray_held")

    # The daemon stops.
    machine.succeed(as_alice("systemctl --user stop ctrlassist-daemon"))
    machine.sleep(3)
    machine.screenshot("tray_no_daemon")
  '';
}
