# A clean NixOS machine: no Steam, Sunshine or other udev rules beyond
# systemd's own. One user is logged in on the seat, so the `uaccess` ACLs a
# real desktop user would get are in place. The test prints its observations
# for the spike's finding; it asserts only that the fixtures work at all.
{
  pkgs,
  probe,
}:
pkgs.testers.runNixOSTest {
  name = "spike-hiding";

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
    services.getty.autologinUser = "alice";
    environment.systemPackages = [
      probe
      pkgs.acl
    ];
  };

  testScript = ''
    import re

    def nodes_of(name):
        """Device nodes of the pad whose name contains `name`, by kind."""
        found = {}
        current = None
        for line in machine.succeed("probe pads").splitlines():
            if line.startswith("pad "):
                current = line
            elif current and name in current:
                match = re.match(r"\s+node (\w+)\s+(\S+)", line)
                if match:
                    found.setdefault(match.group(1), match.group(2))
        return found

    def count(node):
        out = machine.succeed(f"probe listen {node} --millis 500")
        return int(out.strip().split("=")[1])

    def as_alice(command):
        return machine.succeed(f"su -l alice -c '{command}'")

    def report(title, text):
        print(f"\n===== {title} =====\n{text}")

    machine.wait_for_unit("multi-user.target")
    machine.wait_until_succeeds("loginctl list-sessions | grep alice")
    report("sessions", machine.succeed("loginctl list-sessions; loginctl seat-status seat0 | head -5"))

    machine.succeed("systemd-run --unit fakepad probe fake-pad --pulse 20")
    machine.succeed("systemd-run --unit fakehid probe fake-hid --pulse 20")
    machine.wait_until_succeeds("probe pads | grep -c '^pad ' | grep -x 2")
    machine.succeed("udevadm settle")

    report("pads", machine.succeed("probe pads"))
    report("access as alice (seat user, no extra groups)", as_alice("id; probe access"))
    pad = nodes_of("Spike Fake Pad")
    hid = nodes_of("Spike Fake HID Pad")
    report("acls", machine.succeed(
        f"getfacl -p /dev/uinput /dev/uhid {pad['event']} {pad['js']} {hid['event']} {hid['hidraw']} 2>&1"
    ))

    # --- grab -------------------------------------------------------------
    before = count(pad["event"])
    assert before > 0, "the fake pad should be pulsing"
    machine.succeed(f"systemd-run --unit grabber probe grab {pad['event']}")
    machine.wait_until_succeeds("journalctl -u grabber | grep grabbed")
    report("grab", "\n".join([
        f"event reader before grab: {before}",
        f"event reader during grab: {count(pad['event'])}",
        f"js reader during grab:    {count(pad['js'])}",
        "pad still listed during grab: " + str("Spike Fake Pad" in machine.succeed("probe pads")),
        "alice can still open it:\n" + as_alice("probe access | grep -A2 \"Spike Fake Pad\""),
    ]))
    machine.succeed("systemctl kill --signal=KILL grabber")
    machine.wait_until_succeeds(f"test $(probe listen {pad['event']} --millis 500 | cut -d= -f2) -gt 0")
    report("grab after kill -9", f"event reader: {count(pad['event'])}")

    # A grab on the HID pad's event node leaves its hidraw node flowing.
    machine.succeed(f"systemd-run --unit grabber2 probe grab {hid['event']}")
    machine.wait_until_succeeds("journalctl -u grabber2 | grep grabbed")
    report("grab against hidraw", "\n".join([
        f"event reader during grab:  {count(hid['event'])}",
        f"hidraw reader during grab: {count(hid['hidraw'])}",
    ]))
    machine.succeed("systemctl kill --signal=KILL grabber2")

    # --- node permissions (needs root) ------------------------------------
    # A reader that opened the node before it is hidden keeps receiving.
    machine.succeed(f"systemd-run --unit held --uid alice probe listen {pad['event']} --millis 3000")
    machine.wait_until_succeeds("systemctl is-active held")
    machine.succeed(f"setfacl -b {pad['event']} {pad['js']} && chmod 000 {pad['event']} {pad['js']}")
    report("chmod 000 + ACL removed", as_alice("probe access | grep -A2 \"Spike Fake Pad\""))
    machine.wait_until_fails("systemctl is-active held")
    report("reader that opened before the chmod", machine.succeed("journalctl -u held -o cat | grep events="))
    machine.succeed("systemctl restart fakepad")
    machine.wait_until_succeeds("probe pads | grep -c '^pad ' | grep -x 2")
    machine.succeed("udevadm settle")
    report("chmod after the pad reconnects", as_alice("probe access | grep -A2 \"Spike Fake Pad\""))

    # --- runtime udev rule (needs root) -----------------------------------
    machine.succeed(
        "mkdir -p /run/udev/rules.d && "
        "echo 'SUBSYSTEM==\"input\", ATTRS{id/vendor}==\"1209\", ATTRS{id/product}==\"5eed\", "
        "MODE=\"0000\", TAG-=\"uaccess\"' > /run/udev/rules.d/72-spike-hide.rules && "
        "udevadm control --reload && udevadm trigger --subsystem-match=input && udevadm settle"
    )
    report("udev rule, applied to the connected pad", as_alice("probe access | grep -A2 \"Spike Fake Pad\""))
    machine.succeed("systemctl restart fakepad")
    machine.wait_until_succeeds("probe pads | grep -c '^pad ' | grep -x 2")
    machine.succeed("udevadm settle")
    report("udev rule after the pad reconnects", as_alice("probe access | grep -A2 \"Spike Fake Pad\""))
    report("udev rule: does root still read it", f"event reader: {count(nodes_of('Spike Fake Pad')['event'])}")
    machine.succeed(
        "rm /run/udev/rules.d/72-spike-hide.rules && "
        "udevadm control --reload && udevadm trigger --subsystem-match=input && udevadm settle"
    )
    report("udev rule removed", as_alice("probe access | grep -A2 \"Spike Fake Pad\""))
  '';
}
