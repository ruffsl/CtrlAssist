# The daemon, end to end, on a clean machine with one user at a graphical
# session and the hiding service installed: a profile run from real device
# nodes to a virtual pad, with its pads hidden by the real service, through
# a reconnect, a stop, a kill, and a start by itself.
{
  pkgs,
  fixture,
  hider,
  cli,
}:
pkgs.testers.runNixOSTest {
  name = "ctrlassist-daemon";

  nodes.machine = {
    boot.kernelModules = [
      "uinput"
      "uhid"
      "joydev"
    ];
    users.users.alice = {
      isNormalUser = true;
      uid = 1000;
      password = "alice";
    };

    # A graphical session for alice on the seat, as in the hider check.
    services.cage = {
      enable = true;
      user = "alice";
      program = "${pkgs.coreutils}/bin/sleep infinity";
    };
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];

    # The hiding service's units as packaged, and the rule that gives the
    # seat user uinput, which the daemon makes its virtual pads through.
    systemd.packages = [ hider ];
    systemd.sockets.ctrlassist-hider.wantedBy = [ "sockets.target" ];
    systemd.services.ctrlassist-hider.path = [ pkgs.systemd ];
    services.udev.packages = [
      (pkgs.writeTextDir "lib/udev/rules.d/70-ctrlassist.rules" (
        builtins.readFile ../udev/70-ctrlassist.rules
      ))
    ];

    environment.systemPackages = [
      fixture
      hider
      cli
    ];
  };

  testScript = ''
    import json
    import re
    import shlex

    ENV = "XDG_RUNTIME_DIR=/run/user/1000"

    def cli(*arguments, fails=False):
        """Runs the ctrlassist command as alice, against her daemon."""
        command = "su -l alice -c " + shlex.quote(ENV + " ctrlassist " + shlex.join(arguments))
        # What went wrong is said on standard error.
        return machine.fail(command + " 2>&1") if fails else machine.succeed(command)

    def known():
        """The pads the daemon knows, by name."""
        return {pad["name"]: pad for pad in json.loads(cli("pads", "--json"))}

    def setup(name="couch"):
        found = [setup for setup in json.loads(cli("setups", "--json")) if setup["name"] == name]
        return found[0] if found else None

    def phase_is(wanted, name="couch"):
        command = "su -l alice -c " + shlex.quote(ENV + " ctrlassist setups --json")
        machine.wait_until_succeeds(f"{command} | grep -q '\"phase\":\"{wanted}\"'")
        assert setup(name)["phase"] == wanted

    def field(pattern, text):
        match = re.search(pattern, text)
        assert match, f"{pattern!r} not found in {text!r}"
        return match.group(1)

    def nodes(as_user=None):
        """Each pad the fixture lists, by its name, with what the caller can open."""
        command = "ctrlassist-fixture pads"
        if as_user:
            command = f"su -l {as_user} -c '{command}'"
        found = {}
        for line in machine.succeed(command).splitlines():
            quoted = r'"((?:[^"\\]|\\.)*)"'
            name = field("name=" + quoted, line)
            kinds = dict(re.findall(r"(event|js|hidraw)=(\S+)", line))
            found[name] = {kind: tuple(node.rsplit(":", 1)) for kind, node in kinds.items()}
        return found

    def alice_can_open(name, kind="event"):
        return nodes(as_user="alice")[name][kind][1] == "open"

    def rules():
        return machine.succeed("ls /run/udev/rules.d 2>/dev/null | grep ctrlassist || true").split()

    def wait_restored():
        machine.wait_until_succeeds('test -z "$(ls /run/ctrlassist/hidden)"')
        machine.succeed("udevadm settle")
        assert rules() == [], rules()

    def daemon(unit):
        machine.succeed(
            f"systemd-run --unit {unit} --uid alice -p Environment={ENV} -p Environment=HOME=/home/alice "
            "ctrlassist service daemon"
        )
        machine.wait_until_succeeds("su -l alice -c " + shlex.quote(ENV + " ctrlassist status"))

    HID, PLAIN, VIRTUAL = "Fixture Fake HID Pad", "Fixture Fake Pad", "CtrlAssist Virtual Pad"

    machine.wait_for_file("/run/user/1000/wayland-0.lock")
    machine.wait_for_unit("sockets.target")
    # One pad with a HID driver to re-bind, as a PlayStation or Bluetooth
    # pad has, and one without, as an Xbox pad on USB has.
    machine.succeed("systemd-run --unit pad-hid ctrlassist-fixture fake-hid --pulse 20")
    machine.succeed("systemd-run --unit pad-plain ctrlassist-fixture fake-pad --pulse 20")
    machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c '^pad ' | grep -x 2")
    machine.succeed("udevadm settle")
    assert alice_can_open(HID) and alice_can_open(PLAIN)

    daemon("daemon")

    with subtest("the daemon finds the hiding service and the pads"):
        journal = machine.succeed("journalctl -u daemon -o cat")
        assert "the hiding service isn't there" not in journal, journal
        pads = known()
        assert pads[HID]["connected"] and pads[PLAIN]["connected"], pads
        assert pads[HID]["id"] == "address:02:00:00:00:00:01", pads[HID]
        assert not pads[HID]["hidden"] and not pads[PLAIN]["hidden"]
        identities = json.loads(cli("identities", "--json"))
        assert all(identity["available"] for identity in identities), identities

    with subtest("a profile is put, and one that is wrong is refused with where"):
        machine.succeed(
            "printf '%s\\n' 'preset = \"priority\"' 'primary = \"02:00:00:00:00:01\"' "
            "'assist = \"model:1209:5eed\"' > /tmp/couch.toml"
        )
        machine.succeed("printf '%s\\n' 'preset = \"priority\"' > /tmp/wrong.toml")
        assert "preset: " in cli("profile", "validate", "/tmp/wrong.toml", fails=True)
        cli("profile", "put", "/tmp/couch.toml")
        assert "couch" in cli("profile", "list")

    with subtest("started, its pads are hidden by the service and its virtual pad is a seat user's to read"):
        cli("start", "couch")
        phase_is("running")
        pads = known()
        assert pads[HID]["hidden"] and pads[PLAIN]["hidden"], pads
        assert pads[HID]["setup"] == "couch" and pads[PLAIN]["setup"] == "couch"
        machine.succeed("udevadm settle")
        for name in (HID, PLAIN):
            assert not alice_can_open(name, "event") and not alice_can_open(name, "js"), name
        assert len(rules()) == 2, rules()
        assert alice_can_open(VIRTUAL, "event") and alice_can_open(VIRTUAL, "js")

    with subtest("what the hidden pads do reaches a game through the virtual pad"):
        node = setup()["sinks"][0]["node"]
        assert node == nodes()[VIRTUAL]["event"][0], node
        out = machine.succeed(f"su -l alice -c 'ctrlassist-fixture read {node} --frames 6'")
        frames = [line for line in out.splitlines() if line.startswith("frame ")]
        assert len(frames) == 6, out

    with subtest("a hidden pad that reconnects is never visible, and goes back into its setup"):
        machine.succeed("systemctl restart pad-hid")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Fixture Fake HID Pad'")
        machine.succeed("udevadm settle")
        assert not alice_can_open(HID, "event") and not alice_can_open(HID, "js")
        phase_is("running")
        assert all(source["present"] for source in setup()["sources"]), setup()
        assert known()[HID]["hidden"]

    with subtest("stopped, the pads are back as they were and nothing is left behind"):
        cli("stop", "couch")
        assert setup() is None
        machine.succeed("udevadm settle")
        assert alice_can_open(HID, "event") and alice_can_open(PLAIN, "event")
        assert rules() == [], rules()
        machine.succeed('test -z "$(ls /run/ctrlassist/hidden)"')
        assert VIRTUAL not in nodes()
        pads = known()
        assert not pads[HID]["hidden"] and pads[HID].get("setup") is None, pads[HID]

    with subtest("a daemon that is killed gets its pads restored and its virtual pad removed"):
        # Kept with the asking to start by itself, for the daemon after it.
        machine.succeed("printf '%s\\n' 'auto-start = true' >> /tmp/couch.toml")
        cli("profile", "put", "/tmp/couch.toml")
        phase_is("running")
        assert not alice_can_open(HID) and not alice_can_open(PLAIN)
        machine.succeed("systemctl kill --signal=KILL daemon")
        wait_restored()
        assert alice_can_open(HID) and alice_can_open(PLAIN)
        machine.wait_until_fails("ctrlassist-fixture pads | grep -q 'CtrlAssist Virtual Pad'")
        machine.succeed("systemctl reset-failed daemon || true")

    with subtest("the next daemon starts the profile by itself, and stopping the daemon restores the pads"):
        daemon("daemon-again")
        phase_is("running")
        assert known()[HID]["hidden"] and known()[PLAIN]["hidden"]
        assert not alice_can_open(HID) and not alice_can_open(PLAIN)
        machine.succeed("systemctl stop daemon-again")
        wait_restored()
        assert alice_can_open(HID) and alice_can_open(PLAIN)
        assert VIRTUAL not in nodes()
        journal = machine.succeed("journalctl -u daemon-again -o cat")
        assert "the daemon has stopped" in journal, journal
  '';
}
