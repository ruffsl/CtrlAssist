# The settings window as installed by the NixOS module, on a clean
# machine with one user at a graphical session: the session bus starts
# it when asked, as the tray and a launcher ask, it takes its name on
# the bus, and a second launch hands over to it and ends.
{
  pkgs,
  package,
  module,
}:
let
  # What a desktop does as its session starts: tell the session bus, and
  # so what the bus starts, where the compositor is.
  session = pkgs.writeShellScript "session" ''
    ${pkgs.dbus}/bin/dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_RUNTIME_DIR
    exec ${pkgs.coreutils}/bin/sleep infinity
  '';
in
pkgs.testers.runNixOSTest {
  name = "ctrlassist-gui";

  # The check looks at the screen, to know the window was drawn.
  extraPythonPackages = p: [ p.pillow ];

  nodes.machine = {
    imports = [ module ];
    services.ctrlassist = {
      enable = true;
      inherit package;
    };

    users.users.alice = {
      isNormalUser = true;
      uid = 1000;
      password = "alice";
    };

    # A graphical session for alice on the seat, with a compositor the
    # window can open on and software rendering for it to draw with.
    services.cage = {
      enable = true;
      user = "alice";
      program = "${session}";
    };
    hardware.graphics.enable = true;
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];
    # The window, the daemon it talks to and a compositor, all drawing
    # in software.
    virtualisation.memorySize = 2048;
  };

  testScript = ''
    import os
    import shlex
    import tempfile

    from PIL import Image

    NAME = "io.github.ctrlassist.Gui"
    PATH = "/io/github/ctrlassist/Gui"
    ENV = (
        "XDG_RUNTIME_DIR=/run/user/1000 "
        "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus "
        "WAYLAND_DISPLAY=wayland-0"
    )

    def alice(command, fails=False):
        """Runs a command as alice, in her session's environment."""
        command = "su -l alice -c " + shlex.quote(ENV + " " + command)
        return machine.fail(command + " 2>&1") if fails else machine.succeed(command + " 2>&1")

    def windows():
        return int(machine.succeed("pgrep -c -x ctrlassist-gui || true").strip() or 0)

    machine.wait_for_unit("multi-user.target")
    machine.wait_for_file("/run/user/1000/wayland-0")
    machine.wait_for_file("/run/user/1000/bus")

    with subtest("the package puts the window where a launcher and the bus find it"):
        machine.succeed("test -x /run/current-system/sw/bin/ctrlassist-gui")
        entry = machine.succeed(
            "cat /run/current-system/sw/share/applications/io.github.ctrlassist.Gui.desktop"
        )
        assert "DBusActivatable=true" in entry, entry
        assert "Exec=ctrlassist-gui" in entry, entry
        service = machine.succeed(
            "cat /run/current-system/sw/share/dbus-1/services/io.github.ctrlassist.Gui.service"
        )
        assert "/bin/ctrlassist-gui" in service and "/usr/bin" not in service, service

    with subtest("the session bus starts the window when it is asked for"):
        assert windows() == 0
        # What the tray's Settings and a launcher send.
        alice(
            f"busctl --user call {NAME} {PATH} org.freedesktop.Application Activate 'a{{sv}}' 0"
        )
        machine.wait_until_succeeds("pgrep -x ctrlassist-gui")
        owner = alice(f"busctl --user status {NAME}")
        assert "ctrlassist-gui" in owner, owner
        version = alice(f"busctl --user get-property {NAME} {PATH} {NAME}1 Version")
        assert version.startswith('s "'), version
        assert windows() == 1

    with subtest("a second launch hands over to the window that is open, and ends"):
        alice("timeout 20 ctrlassist-gui")
        assert windows() == 1
        alice(
            f"busctl --user call {NAME} {PATH} org.freedesktop.Application Activate 'a{{sv}}' 0"
        )
        assert windows() == 1

    def colors():
        """How many colors the screen shows: one for an empty compositor."""
        with tempfile.TemporaryDirectory() as scratch:
            shot = os.path.join(scratch, "screen.png")
            machine.screenshot(shot)
            return len(Image.open(shot).convert("RGB").getcolors(maxcolors=1 << 24) or [])

    with subtest("the window is drawn"):
        # A sidebar, a list, text and the footer's dots are many colors
        # where the bare compositor is one.
        retry(lambda _last: colors() > 16)
        machine.screenshot("window")
        machine.succeed("pgrep -x ctrlassist-gui")
  '';
}
