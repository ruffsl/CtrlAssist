# What a clean machine gives a seat user: no Steam, Sunshine or other udev
# rules beyond systemd's own. The rest of the design leans on these facts,
# so they are asserted here and a change in the platform fails the check.
{
  pkgs,
  fixture,
}:
pkgs.testers.runNixOSTest {
  name = "ctrlassist-platform";

  nodes.machine = {
    boot.kernelModules = [
      "uinput"
      "uhid"
      "joydev"
    ];
    users.users.alice = {
      isNormalUser = true;
      password = "alice";
    };
    # A session on the seat, so the seat-access ACLs a logged-in user gets
    # are in place.
    services.getty.autologinUser = "alice";
    environment.systemPackages = [ fixture ];
  };

  testScript = ''
    import re

    def field(pattern, text):
        """The first group of a pattern that must match."""
        match = re.search(pattern, text)
        assert match, f"{pattern!r} not found in {text!r}"
        return match.group(1)

    def pads(as_user=None):
        """Each pad the fixture lists, as a dict of its fields and nodes."""
        command = "ctrlassist-fixture pads"
        if as_user:
            command = f"su -l {as_user} -c '{command}'"
        found = {}
        for line in machine.succeed(command).splitlines():
            name = field(r'name="([^"]*)"', line)
            nodes = dict(re.findall(r"(event|js|hidraw)=(\S+)", line))
            found[name] = {kind: tuple(node.rsplit(":", 1)) for kind, node in nodes.items()}
        return found

    def reads(node, millis=500):
        out = machine.succeed(f"ctrlassist-fixture listen {node} --millis {millis}")
        return int(field(r"reads=(\d+) cut_off", out))

    machine.wait_for_unit("multi-user.target")
    machine.wait_until_succeeds("loginctl list-sessions | grep alice")

    machine.succeed("systemd-run --unit fakepad ctrlassist-fixture fake-pad --pulse 20")
    machine.succeed("systemd-run --unit fakehid ctrlassist-fixture fake-hid --pulse 20")
    machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c '^pad ' | grep -x 2")
    machine.succeed("udevadm settle")

    with subtest("a seat user can read pads but can't create virtual ones"):
        machine.fail("su -l alice -c 'test -r /dev/uinput -o -w /dev/uinput'")
        machine.fail("su -l alice -c 'test -r /dev/uhid -o -w /dev/uhid'")
        seen = pads(as_user="alice")
        for name in ("Fixture Fake Pad", "Fixture Fake HID Pad"):
            assert seen[name]["event"][1] == "open", f"{name}: {seen[name]}"
            assert seen[name]["js"][1] == "open", f"{name}: {seen[name]}"

    with subtest("a seat user can't open a pad's hidraw node"):
        assert seen["Fixture Fake HID Pad"]["hidraw"][1] == "denied", seen

    with subtest("a uinput pad has no hidraw node"):
        assert "hidraw" not in seen["Fixture Fake Pad"], seen

    with subtest("both kinds of fake pad deliver what they are sent"):
        seen = pads()
        assert reads(seen["Fixture Fake Pad"]["event"][0]) > 0
        assert reads(seen["Fixture Fake HID Pad"]["event"][0]) > 0
        assert reads(seen["Fixture Fake HID Pad"]["hidraw"][0]) > 0
  '';
}
