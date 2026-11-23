# CtrlAssist as installed by its NixOS module, end to end, on a clean
# machine with one user at a graphical session: the daemon as that user's
# service, a profile run from real device nodes to a virtual pad, with its
# pads hidden by the real hiding service, through a reconnect, a stop, a
# kill, and a start by itself.
{
  pkgs,
  fixture,
  package,
  module,
}:
pkgs.testers.runNixOSTest {
  name = "ctrlassist-daemon";

  nodes.machine = {
    imports = [ module ];
    # Everything CtrlAssist needs of the system comes from this.
    services.ctrlassist = {
      enable = true;
      inherit package;
    };

    # The check's pads are the test crate's fake ones, which a daemon
    # takes for pads only when asked to.
    systemd.user.services.ctrlassist-daemon.environment.CTRLASSIST_TEST_PADS = "1";

    # For the js nodes the checks look at; the module loads the rest.
    boot.kernelModules = [ "joydev" ];
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

    environment.systemPackages = [ fixture ];
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

    def markers():
        """The marker files the hiding service hides pads by."""
        return machine.succeed("cd /run/ctrlassist/hide && find . -type f | sort").split()

    def no_runtime_rules():
        """The hiding service writes no udev rule: the directory for runtime rules never comes to be."""
        machine.succeed("test ! -e /run/udev/rules.d || test -z \"$(ls -A /run/udev/rules.d)\"")

    def wait_restored():
        machine.wait_until_succeeds('test -z "$(ls -A /run/ctrlassist/hide)"')
        machine.succeed("udevadm settle")
        no_runtime_rules()

    def grabbed(name):
        """Whether the seat user, who may open the pad, reads nothing from it: the daemon has it grabbed."""
        node = nodes()[name]["event"][0]
        heard = machine.succeed(f"su -l alice -c 'ctrlassist-fixture listen {node} --millis 1000'")
        return int(field(r"reads=(\d+) cut_off", heard)) == 0

    def as_alice(command):
        return "su -l alice -c " + shlex.quote(ENV + " " + command)

    def daemon_is_up():
        machine.wait_until_succeeds(as_alice("ctrlassist status"))

    def daemon_journal():
        return machine.succeed("journalctl _SYSTEMD_USER_UNIT=ctrlassist-daemon.service -o cat")

    HID, PLAIN, VIRTUAL = "Fixture Fake HID Pad", "Fixture Fake Pad", "CtrlAssist Virtual Pad"

    machine.wait_for_file("/run/user/1000/wayland-0.lock")
    machine.wait_for_unit("sockets.target")
    # One pad with a HID driver to re-bind, as a PlayStation or Bluetooth
    # pad has, and one without, as a uinput pad another program made has:
    # the hiding service refuses that one, and the daemon grabs it.
    machine.succeed("systemd-run --unit pad-hid ctrlassist-fixture fake-hid --pulse 20")
    machine.succeed("systemd-run --unit pad-plain ctrlassist-fixture fake-pad --pulse 20")
    machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c '^pad ' | grep -x 2")
    machine.succeed("udevadm settle")
    assert alice_can_open(HID) and alice_can_open(PLAIN)

    with subtest("the module installs the command, its units, its rule and its completions"):
        machine.succeed("test -x /run/current-system/sw/bin/ctrlassist -a -x /run/current-system/sw/bin/ctrlassist-hider")
        # A shell's completions are linked into the system only where that
        # shell is enabled, so the package itself is looked at.
        for completion in ("fish/vendor_completions.d/ctrlassist.fish", "bash-completion/completions/ctrlassist.bash", "zsh/site-functions/_ctrlassist"):
            machine.succeed("test -s ${package}/share/" + completion)
        machine.succeed("systemctl is-active ctrlassist-hider.socket")
        machine.succeed("su -l alice -c 'test -w /dev/uinput -a -w /dev/uhid'")

    with subtest("the module applies its rules to a pad already connected, as the packages' install script does"):
        # The uevent file of the pad's HID device as the kernel made it,
        # as for a pad connected before the rules were there; the unit a
        # switch to new rules restarts gives it to the service's group again.
        sysname = nodes()[HID]["event"][0].split("/")[-1]
        uevent = f"/sys/class/input/{sysname}/device/device/uevent"
        assert machine.succeed(f"stat -c '%G %a' {uevent}").strip() == "ctrlassist-hider 664"
        machine.succeed(f"chgrp root {uevent} && chmod g-w {uevent}")
        machine.succeed("systemctl restart ctrlassist-udev-trigger.service")
        machine.succeed("udevadm settle")
        assert machine.succeed(f"stat -c '%G %a' {uevent}").strip() == "ctrlassist-hider 664"

    with subtest("the daemon is the user's service, started with the session, and its input thread is real-time"):
        daemon_is_up()
        machine.succeed(as_alice("systemctl --user is-active ctrlassist-daemon.service"))
        assert "real-time scheduling was refused" not in daemon_journal(), daemon_journal()
        # The scheduling class of the daemon's thread named `input`: FF is first-in-first-out.
        machine.succeed("ps -eLo cls,comm | grep -E '^ *FF +input$'")

    with subtest("the tray runs without a tray host, for its notifications"):
        # Its unit hangs off graphical-session.target, which a desktop
        # reaches and this machine's bare compositor doesn't, so it is
        # started by hand. With no tray host it stays up all the same.
        machine.succeed(as_alice("systemctl --user start ctrlassist-tray.service"))
        machine.wait_until_succeeds("journalctl _SYSTEMD_USER_UNIT=ctrlassist-tray.service -o cat | grep -q 'the tray is up'")
        machine.succeed(as_alice("systemctl --user is-active ctrlassist-tray.service"))

    with subtest("the daemon finds the hiding service and the pads"):
        journal = daemon_journal()
        assert "the hiding service isn't there" not in journal, journal
        pads = known()
        assert pads[HID]["connected"] and pads[PLAIN]["connected"], pads
        assert pads[HID]["id"] == "address:02:00:00:00:00:01", pads[HID]
        assert not pads[HID]["hidden"] and not pads[PLAIN]["hidden"]
        identities = {identity["id"]: identity["available"] for identity in json.loads(cli("identities", "--json"))}
        assert identities == {"generic": True, "xbox": True, "dualsense": True}, identities

    with subtest("a profile is put, and one that is wrong is refused with where"):
        machine.succeed(
            "printf '%s\\n' 'mode = \"priority\"' 'primary = \"02:00:00:00:00:01\"' "
            "'assist = \"model:1209:5eed\"' > /tmp/couch.toml"
        )
        machine.succeed("printf '%s\\n' 'mode = \"priority\"' > /tmp/wrong.toml")
        assert "mode: " in cli("profile", "validate", "/tmp/wrong.toml", fails=True)
        cli("profile", "put", "/tmp/couch.toml")
        assert "couch" in cli("profile", "list")

    with subtest("started, its pads are hidden by the service or grabbed, and its virtual pad is a seat user's to read"):
        cli("start", "couch")
        phase_is("running")
        pads = known()
        assert pads[HID]["hidden"] and not pads[PLAIN]["hidden"], pads
        assert pads[HID]["setup"] == "couch" and pads[PLAIN]["setup"] == "couch"
        machine.succeed("udevadm settle")
        assert not alice_can_open(HID, "event") and not alice_can_open(HID, "js")
        assert grabbed(PLAIN), "the pad the service refused is grabbed"
        assert markers(), "the hidden pad has its markers"
        no_runtime_rules()
        # The service the daemon is connected to is the installed one: its
        # own user, no capabilities.
        pid = machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip()
        status = machine.succeed(f"cat /proc/{pid}/status")
        assert field(r"Uid:\s+(\d+)", status) != "0", status
        assert field(r"CapEff:\s+(\w+)", status) == "0000000000000000", status
        assert field(r"CapBnd:\s+(\w+)", status) == "0000000000000000", status
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

    with subtest("a profile of the same pads is taken as the setup runs, on the virtual pad it has"):
        node = setup()["sinks"][0]["node"]
        machine.succeed("sed -i 's/priority/toggle/' /tmp/couch.toml")
        cli("profile", "put", "/tmp/couch.toml")
        assert "would apply without" in cli("update", "couch", "--dry-run")
        assert "applied without" in cli("update", "couch")
        command = "su -l alice -c " + shlex.quote(ENV + " ctrlassist setups --json")
        machine.wait_until_succeeds(f"{command} | grep -q '\"choices\":\\[\"primary\",\"assist\"\\]'")
        taken = setup()
        assert taken["phase"] == "running" and taken["sinks"][0]["node"] == node, taken
        assert node == nodes()[VIRTUAL]["event"][0], node
        assert known()[HID]["hidden"] and not known()[PLAIN]["hidden"]
        assert markers()

    with subtest("its pads change parts as it runs, staying hidden"):
        machine.succeed(
            "printf '%s\\n' 'mode = \"priority\"' 'assist = \"02:00:00:00:00:01\"' "
            "'primary = \"model:1209:5eed\"' > /tmp/couch.toml"
        )
        cli("profile", "put", "/tmp/couch.toml")
        was = {source["name"]: source["pad"] for source in setup()["sources"]}
        assert "applied without" in cli("update", "couch")
        now = {source["name"]: source["pad"] for source in setup()["sources"]}
        assert now == {"primary": was["assist"], "assist": was["primary"]}, (was, now)
        taken = setup()
        assert taken["phase"] == "running" and taken["sinks"][0]["node"] == node, taken
        assert all(source["present"] for source in taken["sources"]), taken
        assert known()[HID]["hidden"] and not known()[PLAIN]["hidden"]
        assert markers()
        machine.succeed(
            "printf '%s\\n' 'mode = \"priority\"' 'primary = \"02:00:00:00:00:01\"' "
            "'assist = \"model:1209:5eed\"' > /tmp/couch.toml"
        )

    with subtest("stopped, the pads are back as they were and nothing is left behind"):
        cli("stop", "couch")
        assert setup() is None
        # The stop is answered once the virtual pad is gone; the pad the
        # service hid is shown again after, and said to be hidden until
        # the service has said it is shown.
        retry(lambda _last: not known()[HID]["hidden"])
        machine.succeed("udevadm settle")
        assert alice_can_open(HID, "event") and alice_can_open(PLAIN, "event")
        assert markers() == [], markers()
        no_runtime_rules()
        assert VIRTUAL not in nodes()
        pads = known()
        assert not pads[HID]["hidden"] and pads[HID].get("setup") is None, pads[HID]
        # Shown again, the pad with a driver went and came back as the
        # driver was re-bound. It never left, as far as anyone is told:
        # it comes back, and nothing says it went, nor that it connected.
        machine.wait_until_succeeds(
            "journalctl _SYSTEMD_USER_UNIT=ctrlassist-daemon.service -o cat | tac | sed '/a setup stopped/q' | grep -q 'a pad is back from a re-bind of its driver'"
        )
        since_stop = daemon_journal().split("a setup stopped")[-1]
        assert "a pad disconnected" not in since_stop and "a pad connected" not in since_stop, since_stop
        assert known()[HID]["connected"] and known()[PLAIN]["connected"], known()

    with subtest("udev sends a re-bind the service asks on before the removals it causes, as the daemon hears them"):
        # The device crate's monitor, as the daemon's watch reads it, says
        # each change in the order udev sent it, through a hide and a
        # showing again of the pad with a HID driver.
        machine.succeed("systemd-run --unit watch -p StandardOutput=file:/tmp/watch ctrlassist-fixture watch")
        machine.wait_until_succeeds("grep -qx watching /tmp/watch")
        node = nodes()[HID]["event"][0]
        anchor = machine.succeed(f"readlink -f /sys/class/input/{node.split('/')[-1]}/device/device").strip()
        cli("start", "couch")
        phase_is("running")
        machine.succeed("udevadm settle")
        cli("stop", "couch")
        retry(lambda _last: not known()[HID]["hidden"])
        machine.succeed("udevadm settle")
        machine.succeed("systemctl stop watch")
        heard = machine.succeed("cat /tmp/watch").splitlines()[1:]
        # The pad's own: the requests on its HID device, the arrivals
        # under it, and the removals of the nodes it had.
        ours, order, present = {node}, [], []
        for line in heard:
            word = line.split()
            if word[0] == "rebinding" and word[1] == anchor:
                order.append("rebinding")
                present = word[2].removeprefix("present=").split(",")
            elif word[0] == "arrived" and word[3] == f"anchor={anchor}":
                if order[-1:] != ["arrived"]:
                    # The rule re-bound the driver before udev sent the
                    # request on: the node it made was there to be seen.
                    assert word[2] in present, (line, present, heard)
                    order.append("arrived")
                ours.add(word[1])
            elif word[0] == "left" and word[1] in ours and order[-1:] != ["left"]:
                order.append("left")
        print("\n".join(heard))
        assert order == ["rebinding", "left", "arrived"] * 2, (order, heard)

    with subtest("udev's events lost to a watch not read in time are said once what was kept is read"):
        # The kernel keeps what was queued, drops what comes after until
        # the queue has been read empty, and says so before the first it
        # kept is read: the monitor says the loss after what it kept, which
        # came before the loss, so that what is connected is looked at
        # again after those and not before. Its buffer is the least the
        # kernel gives, and it is stopped while udev sends it more.
        machine.succeed("systemd-run --unit overrun -p StandardOutput=file:/tmp/overrun ctrlassist-fixture watch --receive-buffer 1")
        machine.wait_until_succeeds("grep -qx watching /tmp/overrun")
        machine.succeed("systemctl kill --signal=SIGSTOP overrun")
        first = nodes()[HID]["event"][0]
        machine.succeed(f"udevadm trigger --settle --action=change /sys/class/input/{first.split('/')[-1]}")
        machine.succeed("udevadm trigger --settle --action=change --subsystem-match=input")
        machine.succeed("systemctl kill --signal=SIGCONT overrun")
        machine.wait_until_succeeds("grep -qx overran /tmp/overrun")
        machine.succeed("systemctl stop overrun")
        heard = machine.succeed("cat /tmp/overrun").splitlines()[1:]
        print("\n".join(heard))
        assert heard[0].startswith(f"arrived {first} "), heard
        assert heard[-1] == "overran" and heard.count("overran") == 1, heard

    with subtest("a connected DualSense is copied through the service, and virtual ones are made from it"):
        # A stand-in for a real one: a pad of the kernel's PlayStation
        # driver with an address a real pad could have.
        machine.succeed("systemd-run --unit pad-ds ctrlassist-fixture fake-dualsense --real --pulse 20")
        machine.succeed("su -l alice -c 'test ! -e ~/.local/state/ctrlassist/identities'")
        machine.wait_until_succeeds("su -l alice -c 'test -s ~/.local/state/ctrlassist/identities/dualsense.toml'")
        # The copy is of the stand-in, whose descriptor isn't the shipped one.
        machine.succeed("su -l alice -c 'grep -q ^descriptor.*0600ff0901a101 ~/.local/state/ctrlassist/identities/dualsense.toml'")
        machine.succeed(
            "printf '%s\\n' 'mode = \"multicast\"' 'pad = \"02:ca:7e:00:00:01\"' "
            "'identity = \"dualsense\"' > /tmp/twin.toml"
        )
        cli("profile", "put", "/tmp/twin.toml")
        cli("start", "twin")
        phase_is("running", name="twin")
        twin = setup("twin")
        assert [sink["identity"] for sink in twin["sinks"]] == ["dualsense", "dualsense"], twin
        machine.succeed("udevadm settle")
        # Each is the kernel driver's own pad, the user's to read, and
        # carries what the pad behind it does; none is taken for a pad.
        for sink in twin["sinks"]:
            out = machine.succeed(f"su -l alice -c 'ctrlassist-fixture read {sink['node']} --frames 4'")
            assert len([line for line in out.splitlines() if line.startswith("frame ")]) == 4, out
        assert not any("02:ca:55:" in pad["id"] for pad in json.loads(cli("pads", "--json")))
        machine.succeed("test $(ls /sys/bus/hid/drivers/playstation | grep -c 054C:0CE6) = 3")
        cli("stop", "twin")
        assert setup("twin") is None
        machine.succeed("systemctl stop pad-ds")
        machine.wait_until_succeeds("test $(ls /sys/bus/hid/drivers/playstation | grep -c 054C:0CE6) = 0")
        cli("profile", "delete", "twin")

    with subtest("a daemon that is killed gets its pads restored and its virtual pad removed"):
        # Kept with the asking to start by itself, for the daemon after it.
        machine.succeed("printf '%s\\n' 'managed = true' >> /tmp/couch.toml")
        cli("profile", "put", "/tmp/couch.toml")
        phase_is("running")
        assert not alice_can_open(HID) and grabbed(PLAIN)
        machine.succeed(as_alice("systemctl --user kill --signal=KILL ctrlassist-daemon.service"))
        # The hiding service restores at once; the unit brings the daemon
        # back only after its restart delay, so there is time to look.
        wait_restored()
        assert alice_can_open(HID) and alice_can_open(PLAIN) and not grabbed(PLAIN)
        assert VIRTUAL not in nodes()

    with subtest("the unit restarts the daemon, which starts the profile by itself"):
        daemon_is_up()
        phase_is("running")
        assert known()[HID]["hidden"] and not known()[PLAIN]["hidden"]
        assert not alice_can_open(HID) and grabbed(PLAIN)

    with subtest("a profile that starts by itself stays stopped when stopped by hand"):
        # Showing its pads again re-binds their drivers, so they go and
        # come back: that is not a pad plugged in, and starts nothing.
        cli("stop", "couch")
        machine.succeed("udevadm settle")
        machine.wait_until_succeeds('test -z "$(ls -A /run/ctrlassist/hide)"')
        machine.succeed("udevadm settle")
        assert setup() is None, setup()
        assert "a profile starts by itself" not in daemon_journal().split("a setup stopped")[-1]
        cli("start", "couch")
        phase_is("running")

    with subtest("a pad re-bound by anyone but the service has left and is plugged in, which starts a profile that starts by itself"):
        # As a pad on a wireless receiver switched off and on again: its
        # node goes and comes back while the device above it stays, with
        # no request from the service.
        cli("stop", "couch")
        retry(lambda _last: not known()[HID]["hidden"])
        machine.wait_until_succeeds('test -z "$(ls -A /run/ctrlassist/hide)"')
        machine.succeed("udevadm settle")
        assert setup() is None, setup()
        node = nodes()[HID]["event"][0]
        hid = machine.succeed(f"basename $(readlink -f /sys/class/input/{node.split('/')[-1]}/device/device)").strip()
        machine.succeed(f"echo -n {hid} > /sys/bus/hid/devices/{hid}/driver/unbind")
        retry(lambda _last: not known()[HID]["connected"])
        since_stop = daemon_journal().split("a setup stopped")[-1]
        assert since_stop.count("a pad disconnected") == 1, since_stop
        machine.succeed(f"echo -n {hid} > /sys/bus/hid/drivers_probe")
        phase_is("running")
        assert known()[HID]["connected"] and known()[HID]["hidden"], known()
        # The start hid the pad, a re-bind of the service's own: not
        # published as the pad leaving.
        machine.succeed("udevadm settle")
        since_stop = daemon_journal().split("a setup stopped")[-1]
        assert since_stop.count("a pad disconnected") == 1, since_stop
        assert "a profile starts by itself" in since_stop, since_stop

    with subtest("stopping the service restores the pads"):
        machine.succeed(as_alice("systemctl --user stop ctrlassist-daemon.service"))
        wait_restored()
        assert alice_can_open(HID) and alice_can_open(PLAIN)
        assert VIRTUAL not in nodes()
        assert "the daemon has stopped" in daemon_journal()
  '';
}
