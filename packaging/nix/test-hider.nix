# The hiding service, end to end, on a clean machine with one user at a
# graphical session: hide a pad, lose and regain it, restore it, and every
# way of ending that isn't a polite restore.
{
  pkgs,
  fixture,
  hider,
}:
pkgs.testers.runNixOSTest {
  name = "ctrlassist-hider";

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

    # A graphical session for alice on the seat: the service serves only
    # the user in front of the machine, and a console login doesn't count.
    services.cage = {
      enable = true;
      user = "alice";
      program = "${pkgs.coreutils}/bin/sleep infinity";
    };
    # The default display adapter has no driver cage can use.
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];

    # The units as packaged, hardening included.
    systemd.packages = [ hider ];
    systemd.sockets.ctrlassist-hider.wantedBy = [ "sockets.target" ];
    # For udevadm.
    systemd.services.ctrlassist-hider.path = [ pkgs.systemd ];

    environment.systemPackages = [
      fixture
      hider
    ];
  };

  testScript = ''
    import re
    import shlex

    SOCKET = "/run/ctrlassist/hider.sock"

    def field(pattern, text):
        """The first group of a pattern that must match."""
        match = re.search(pattern, text)
        assert match, f"{pattern!r} not found in {text!r}"
        return match.group(1)

    def pads(as_user=None):
        """Each pad the fixture lists, by its address or else its name."""
        command = "ctrlassist-fixture pads"
        if as_user:
            command = f"su -l {as_user} -c '{command}'"
        found = {}
        for line in machine.succeed(command).splitlines():
            # The fixture prints strings quoted, with any quote inside escaped.
            quoted = r'"((?:[^"\\]|\\.)*)"'
            key = field("uniq=" + quoted, line) or field("name=" + quoted, line)
            key = re.sub(r"\\(.)", r"\1", key)
            nodes = dict(re.findall(r"(event|js|hidraw)=(\S+)", line))
            found[key] = {kind: tuple(node.rsplit(":", 1)) for kind, node in nodes.items()}
        return found

    def alice_can_open(key, kind="event"):
        return pads(as_user="alice")[key][kind][1] == "open"

    def fake(unit, *arguments):
        machine.succeed("systemd-run --unit " + unit + " ctrlassist-fixture " + shlex.join(arguments))

    def hide(unit, node, user="alice"):
        """Starts a client that hides `node` and holds; returns its control directory and what it was told."""
        control = f"/tmp/{unit}"
        machine.succeed(
            f"systemd-run --unit {unit} --uid {user} ctrlassist-fixture hide {node} --socket {SOCKET} --control {control}"
        )
        machine.wait_for_file(f"{control}/hide.done")
        return control, machine.succeed(f"cat {control}/hide.done")

    def order(control, what):
        machine.succeed(f"rm -f {control}/{what}.done; touch {control}/{what}")
        machine.wait_for_file(f"{control}/{what}.done")
        return machine.succeed(f"cat {control}/{what}.done")

    def wait_restored():
        """Waits until nothing is hidden. The record is the last thing the service removes."""
        machine.wait_until_succeeds('test -z "$(ls /run/ctrlassist/hidden)"')
        machine.succeed("udevadm settle")
        assert rules() == [], rules()

    def rules():
        return machine.succeed("ls /run/udev/rules.d 2>/dev/null | grep ctrlassist || true").split()

    A, B = "02:00:00:00:00:01", "02:00:00:00:00:02"

    machine.wait_for_file("/run/user/1000/wayland-0.lock")
    machine.wait_for_unit("sockets.target")
    # Two pads of the same model, told apart only by their addresses.
    fake("pad-a", "fake-hid", "--uniq", A, "--pulse", "20")
    fake("pad-b", "fake-hid", "--uniq", B, "--pulse", "20")
    machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c '^pad ' | grep -x 2")
    machine.succeed("udevadm settle")
    assert alice_can_open(A) and alice_can_open(B)

    with subtest("the service tells a pad from what isn't one"):
        machine.succeed(f"ctrlassist-hider check {pads()[A]['event'][0]} | grep '^pad '")
        keyboard = machine.succeed("ls /dev/input/by-path/*-kbd | head -1").strip()
        machine.succeed(f"ctrlassist-hider check {keyboard} | grep 'refused'")
        machine.succeed("ctrlassist-hider check /dev/null | grep 'refused'")

    with subtest("a raw node is opened only for the kinds of pad the service names"):
        raw = f"ctrlassist-fixture raw {pads()[A]['event'][0]} --socket {SOCKET}"
        told = machine.succeed(f"su -l alice -c '{raw}'")
        assert "doesn't open that kind of pad's raw node" in told, told
        assert "isn't the user in front" in machine.succeed(raw)

    with subtest("someone not in front of the machine is refused"):
        _, told = hide("as-root", pads()[A]["event"][0], user="root")
        assert "isn't the user in front" in told, told
        # The refused client has exited by itself.
        machine.wait_until_fails("systemctl is-active as-root")
        assert alice_can_open(A)

    with subtest("hiding denies the pad and cuts off whoever had it open"):
        node = pads()[A]["event"][0]
        machine.succeed(f"systemd-run --unit early --uid alice ctrlassist-fixture listen {node} --millis 60000")
        machine.wait_until_succeeds("journalctl -u early | grep 't=1s'")
        control, told = hide("client", node)
        assert told.startswith("ok") and "dislodged=true" in told, told
        machine.wait_until_fails("systemctl is-active early")
        assert "cut_off=true" in machine.succeed("journalctl -u early -o cat"), "the early reader kept reading"
        assert not alice_can_open(A, "event") and not alice_can_open(A, "js")
        assert len(rules()) == 1, rules()

    with subtest("the other pad of the same model is untouched"):
        assert alice_can_open(B, "event") and alice_can_open(B, "js")

    with subtest("the descriptor the service passed delivers the pad's input"):
        assert int(field(r"reads=(\d+)", order(control, "count"))) > 0

    with subtest("a pad that reconnects is hidden before anyone can see it"):
        machine.succeed("systemctl restart pad-a")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c '^pad ' | grep -x 2")
        machine.succeed("udevadm settle")
        assert not alice_can_open(A, "event") and not alice_can_open(A, "js")
        assert order(control, "reopen").startswith("ok")
        assert int(field(r"reads=(\d+)", order(control, "count"))) > 0

    with subtest("restoring gives the pad back and leaves nothing behind"):
        assert order(control, "restore").startswith("ok")
        machine.succeed("udevadm settle")
        assert alice_can_open(A, "event") and alice_can_open(A, "js")
        assert rules() == [], rules()
        machine.succeed("test -z \"$(ls /run/ctrlassist/hidden)\"")
        machine.succeed("systemctl stop client")

    with subtest("a client that is killed gets its pads restored"):
        control, told = hide("doomed", pads()[A]["event"][0])
        assert told.startswith("ok"), told
        assert not alice_can_open(A)
        machine.succeed("systemctl kill --signal=KILL doomed")
        wait_restored()
        assert alice_can_open(A)

    with subtest("a service that is killed doesn't leave pads hidden"):
        control, told = hide("orphaned", pads()[A]["event"][0])
        assert told.startswith("ok"), told
        machine.succeed("systemctl kill --signal=KILL ctrlassist-hider.service")
        wait_restored()
        assert alice_can_open(A)
        machine.succeed("systemctl stop orphaned; systemctl reset-failed ctrlassist-hider.service || true")

    with subtest("a pad's own name and address can't reach a rule"):
        evil = 'evil", RUN+="/bin/touch /tmp/pwned'
        fake("pad-evil", "fake-hid", "--name", evil, "--uniq", evil, "--product", "0bad", "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c '^pad ' | grep -x 3")
        machine.succeed("udevadm settle")
        control, told = hide("hostile", pads()[evil]["event"][0])
        assert told.startswith("ok"), told
        assert not alice_can_open(evil)
        machine.succeed("! grep -r RUN /run/udev/rules.d/")
        machine.succeed("test ! -e /tmp/pwned")
        assert order(control, "restore").startswith("ok")
        machine.succeed("systemctl stop hostile pad-evil")

    with subtest("a pad with no driver is hidden from new readers and restored exactly"):
        fake("pad-plain", "fake-pad", "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Fixture Fake Pad'")
        machine.succeed("udevadm settle")
        plain = "Fixture Fake Pad"
        node = pads()[plain]["event"][0]
        before = machine.succeed(f"stat -c '%a %U:%G' {node}")
        control, told = hide("plain", node)
        assert told.startswith("ok") and "dislodged=false" in told, told
        assert not alice_can_open(plain, "event") and not alice_can_open(plain, "js")
        assert int(field(r"reads=(\d+)", order(control, "count"))) > 0
        assert order(control, "restore").startswith("ok")
        machine.succeed("udevadm settle")
        assert alice_can_open(plain, "event") and alice_can_open(plain, "js")
        assert machine.succeed(f"stat -c '%a %U:%G' {node}") == before
  '';
}
