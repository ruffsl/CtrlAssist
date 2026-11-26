# The device crate against the kernel on a clean machine with the packaged
# udev rule: a seat user reads a pad and makes virtual pads, as the daemon
# will.
{
  pkgs,
  fixture,
}:
pkgs.testers.runNixOSTest {
  name = "ctrlassist-device";

  nodes.machine = {
    # The first release whose hid-steam knows the 2026 Steam Controller,
    # from the binary cache, so that a fake one is bound here as nowhere
    # older can.
    boot.kernelPackages = pkgs.linuxPackages_7_3;
    boot.kernelModules = [
      "uinput"
      "uhid"
      "hid-playstation"
      "hid-steam"
      "joydev"
    ];
    # Says this is a virtual machine check, where a fake that can type and
    # point troubles nobody.
    environment.etc."ctrlassist-vm-check".text = "";
    users.users.alice = {
      isNormalUser = true;
      uid = 1000;
      password = "alice";
    };
    services.getty.autologinUser = "alice";
    # The packaged rule that gives the seat user uinput and uhid.
    services.udev.packages = [
      (pkgs.writeTextDir "lib/udev/rules.d/70-ctrlassist.rules" (
        builtins.readFile ../udev/70-ctrlassist.rules
      ))
    ];
    environment.systemPackages = [ fixture ];
  };

  testScript = ''
    machine.wait_for_unit("multi-user.target")
    # A seat user: logged in on the console, as in the platform check.
    machine.wait_until_succeeds("loginctl list-sessions | grep alice")

    machine.succeed("systemd-run --unit pad ctrlassist-fixture fake-pad --pulse 20")
    machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c '^pad '")
    machine.succeed("udevadm settle")
    node = machine.succeed("ctrlassist-fixture pads | grep -o 'event=[^: ]*' | cut -d= -f2").strip()

    with subtest("a seat user reads a pad's frames from a descriptor"):
        out = machine.succeed(f"su -l alice -c 'ctrlassist-fixture read {node} --frames 6'")
        lines = out.splitlines()
        assert lines[0].startswith('pad name="Fixture Fake Pad" id=1209:5eed'), lines[0]
        frames = [line for line in lines if line.startswith("frame ")]
        assert len(frames) == 6, out
        # The fake pad presses and releases one button: each state differs
        # from the one before, and both values are seen.
        presses = [line.split()[1] for line in frames]
        assert all(a != b for a, b in zip(presses, presses[1:])), out
        assert set(presses) == {"south=1", "south=0"}, out

    with subtest("the packaged rule gives the seat user uinput and uhid, and nobody else"):
        machine.succeed("su -l alice -c 'test -w /dev/uinput -a -w /dev/uhid'")
        machine.fail("su -s /bin/sh nobody -c 'test -w /dev/uinput -o -w /dev/uhid'")

    with subtest("a seat user's virtual pad carries what the pad behind it does"):
        for unit, flag, name in [("generic", "", "CtrlAssist Virtual Pad"), ("xbox", " --xbox", "Microsoft Xbox Series")]:
            machine.succeed(f"systemd-run --unit {unit} --uid alice ctrlassist-fixture forward {node}{flag}")
            machine.wait_until_succeeds(f"journalctl -u {unit} -o cat | grep '^virtual node='")
            virtual = machine.succeed(f"journalctl -u {unit} -o cat | grep -o 'node=.*' | cut -d= -f2").strip()
            machine.succeed("udevadm settle")
            out = machine.succeed(f"su -l alice -c 'ctrlassist-fixture read {virtual} --frames 4'")
            assert name in out.splitlines()[0], out
            presses = [line.split()[1] for line in out.splitlines() if line.startswith("frame ")]
            assert len(presses) == 4 and all(a != b for a, b in zip(presses, presses[1:])), out
            # A game sees it as a joystick with a js node of its own.
            machine.succeed(f"udevadm info {virtual} | grep ID_INPUT_JOYSTICK=1")
            machine.succeed(f"systemctl stop {unit}")

    with subtest("a virtual DualSense's sensors and touchpad carry what it is written"):
        import re
        # It reports a reading at each pulse, the two of `fake-dualsense
        # --motion` by turns, each 4 ms after the last by its own clock.
        machine.succeed("systemd-run --unit ds ctrlassist-fixture fake-dualsense --pulse 50 --motion")
        def node_named(name):
            return machine.succeed(
                "for d in /sys/class/input/event*; do "
                f"if grep -qx 'Sony Interactive Entertainment DualSense Wireless Controller {name}' $d/device/name; "
                "then echo /dev/input/$(basename $d); fi; done"
            ).strip()
        machine.wait_until_succeeds("ls /sys/class/input/event*/device/name | xargs grep -l 'Motion Sensors'")
        machine.wait_until_succeeds("ls /sys/class/input/event*/device/name | xargs grep -l 'Touchpad'")
        motion, touchpad = node_named("Motion Sensors"), node_named("Touchpad")
        assert motion and touchpad, (motion, touchpad)
        # evtest reads what the kernel's PlayStation driver makes of each
        # report, line by line, as the reports come.
        for unit, read in [("motion-events", motion), ("touch-events", touchpad)]:
            machine.succeed(f"systemd-run --unit {unit} stdbuf -oL ${pkgs.evtest}/bin/evtest {read}")
            machine.wait_until_succeeds(f"test $(journalctl -u {unit} -o cat | grep -c SYN_REPORT) -ge 12")

        def frames(unit):
            """What evtest read, frame by frame: each a list of (type, code, value), by number."""
            found, frame = [], []
            for line in machine.succeed(f"journalctl -u {unit} -o cat").splitlines():
                event = re.match(r"Event: time [\d.]+, type (\d+) \([^)]*\), code (\d+) \([^)]*\), value (-?\d+)", line)
                if event:
                    frame.append(tuple(int(part) for part in event.groups()))
                elif line.startswith("Event: ") and "SYN_REPORT" in line:
                    found.append(frame)
                    frame = []
            return found

        # The driver's motion node: gravity at 8192 to the g, exactly, and
        # the rate at 1024 to the °/s, from counts at 17678 to 1080 °/s,
        # each to within a count or two. MSC_TIMESTAMP is the virtual
        # pad's clock in µs: the k-th reading at 4000k, whatever the pulse.
        EV_ABS, EV_MSC, EV_KEY = 3, 4, 1
        gravity = {True: [2048, -8192, 20480], False: [-4096, 6144, -31949]}
        rate = {True: [12.5, -250.0, 1000.0], False: [-0.5, 90.0, -1999.0]}
        count = 1080 / 17678 * 1024
        stamps = []
        for frame in frames("motion-events"):
            [stamp] = [value for kind, code, value in frame if (kind, code) == (EV_MSC, 5)]
            stamps.append(stamp)
            pressed = (stamp // 4000) % 2 == 1
            axes = {code: value for kind, code, value in frame if kind == EV_ABS}
            assert sorted(axes) == [0, 1, 2, 3, 4, 5], frame
            for axis in range(3):
                assert abs(axes[axis] - gravity[pressed][axis]) <= 2, (axis, frame)
                assert abs(axes[3 + axis] - rate[pressed][axis] * 1024) <= 2 * count, (3 + axis, frame)
        assert len(stamps) >= 12 and all(stamp % 4000 == 0 for stamp in stamps), stamps
        assert all(b - a == 4000 for a, b in zip(stamps, stamps[1:])), stamps

        # The touchpad's node: the click as BTN_LEFT, and each contact at
        # its place across 0 to 1919 and down 0 to 1079. A finger put down
        # is given a tracking id of its own, and lifted ends it.
        slot, first_ids, second_ids, places = None, set(), [], []
        clicks = []
        for frame in frames("touch-events"):
            seen = {}
            for kind, code, value in frame:
                if (kind, code) == (EV_ABS, 47):
                    slot = value
                elif kind == EV_ABS and code in (53, 54, 57):
                    seen[(slot, code)] = value
                elif (kind, code) == (EV_KEY, 272):
                    clicks.append(value)
            if clicks and clicks[-1] == 1:
                assert (seen[(0, 53)], seen[(0, 54)]) == (480, 540), frame
                assert seen[(1, 57)] >= 0 and seen[(1, 57)] not in second_ids, frame
                second_ids.append(seen[(1, 57)])
                places.append((seen[(1, 53)], seen[(1, 54)]))
            else:
                assert (seen[(0, 53)], seen[(0, 54)]) == (960, 0), frame
                assert seen[(1, 57)] == -1, frame
            if (0, 57) in seen:
                first_ids.add(seen[(0, 57)])
        assert len(clicks) >= 12 and all(a != b for a, b in zip(clicks, clicks[1:])), clicks
        assert len(first_ids) <= 1 and -1 not in first_ids, first_ids
        assert set(places) == {(1439, 1079), (1919, 809)}, places
        assert all(a != b for a, b in zip(places, places[1:])), places
        machine.succeed("systemctl stop motion-events touch-events ds")

    with subtest("a fake Steam Controller is bound by hid-steam and read through its raw node"):
        import re
        machine.succeed("systemd-run --unit steam ctrlassist-fixture fake-steam --pulse 20")
        machine.wait_until_succeeds("journalctl -u steam -o cat | grep '^raw '")
        def steam_log(unit="steam"):
            return machine.succeed(f"journalctl -u {unit} -o cat")
        def said(key, unit="steam"):
            """What the fixture said `key` was, in the unit's log."""
            found = re.search(rf"^{key} (\S+)$", steam_log(unit), re.M)
            assert found, (key, unit)
            return found.group(1)
        serial, phys, gamepad, motion, raw = (said(key) for key in ["serial", "phys", "gamepad", "motion", "raw"])
        assert serial.startswith("CATEST") and phys.startswith("ctrlassist/test/steam/"), (serial, phys)
        machine.succeed("udevadm settle")
        # The driver's gamepad and motion sensors, each with the fake's
        # serial as uniq and its phys, which marks it as the tests'.
        for event, name in [(gamepad, "Steam Controller"), (motion, "Steam Controller Motion Sensors")]:
            device = f"/sys/class/input/{event.split('/')[-1]}/device"
            assert machine.succeed(f"cat {device}/name").strip() == name, event
            assert machine.succeed(f"cat {device}/uniq").strip() == serial, event
            assert machine.succeed(f"cat {device}/phys").strip() == phys, event
        machine.succeed(f"udevadm info {gamepad} | grep -x 'E: ID_INPUT_JOYSTICK=1'")
        machine.succeed(f"udevadm info {motion} | grep -x 'E: ID_INPUT_ACCELEROMETER=1'")
        machine.succeed(f"ctrlassist-fixture pads | grep 'name=\"Steam Controller\" uniq=\"{serial}\" id=28de:1302'")
        # Its battery, named by the serial, which is short enough to name
        # the battery's thermal zone too.
        machine.succeed(f"test -d /sys/class/power_supply/steam-{serial}")
        # It asked the serial and the attributes, each a SET then a GET,
        # which the fake answered at once.
        log = steam_log()
        assert re.search(r"^received set-feature 01ae1601", log, re.M), log
        assert re.search(r"^received set-feature 0183", log, re.M), log
        assert log.count("received get-feature 1") >= 2, log

        # The raw node is the client device's, root's alone without a rule
        # like Steam's; root opens it, and the driver takes its input
        # devices away while it is open.
        machine.fail(f"su -l alice -c 'test -r {raw}'")
        machine.succeed(
            f"systemd-run --unit steam-raw ctrlassist-fixture hidraw {raw} --reads 6 "
            "--write 80000040001000002000 --set-feature 0187030900 --hold"
        )
        machine.wait_until_succeeds("test $(journalctl -u steam-raw -o cat | grep -c '^read ') -ge 6")
        machine.wait_until_fails(f"ctrlassist-fixture pads | grep 'uniq=\"{serial}\"'")
        machine.fail(f"grep -lx '{phys}' /sys/class/input/input*/phys")
        out = machine.succeed("journalctl -u steam-raw -o cat")
        # Every report read is one the fake sent, byte for byte. The fake
        # says what it sent once the send has returned, which can be after
        # the reader has said it read it: each is waited for.
        reads = re.findall(r"^read (\S+)$", out, re.M)
        assert len(reads) >= 6, out
        assert all(read.startswith("42") and len(read) == 108 for read in reads), reads
        for read in reads:
            machine.wait_until_succeeds(f"journalctl -u steam -o cat | grep -qx 'sent {read}'")
        # What was written reached the fake as it was written: the output
        # report as it is, the feature report padded to 64 bytes, and the
        # fake's echo of it came back.
        feature = "0187030900" + "00" * 59
        machine.wait_until_succeeds("journalctl -u steam -o cat | grep -x 'received output 80000040001000002000'")
        machine.wait_until_succeeds(f"journalctl -u steam -o cat | grep -x 'received set-feature {feature}'")
        assert re.search(rf"^get-feature {feature}$", out, re.M), out

        # Closed, the driver makes them anew and asks the serial again.
        asked = steam_log().count("received set-feature 01ae")
        machine.succeed("systemctl stop steam-raw")
        machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep 'uniq=\"{serial}\"'")
        machine.wait_until_succeeds(f"test $(journalctl -u steam -o cat | grep -c '^received set-feature 01ae') -gt {asked}")
        machine.succeed("systemctl stop steam")
        machine.wait_until_fails(f"grep -lx '{phys}' /sys/class/input/input*/phys")

    with subtest("hid-steam reads a fake Steam Controller's state as the fake lays it out, with lizard_mode off and on"):
        # `SteamState::busy`, as the driver presents it: south and two back
        # buttons held; the sticks' Y the report's inverted; the left
        # trackpad where the report says, touched, the right at 0,
        # untouched; the triggers on HAT2Y and HAT2X as they are; and the
        # accelerometer's and gyroscope's second axis as Z, inverted, and
        # their third as Y.
        BTN_SOUTH, BTN_GRIPR, BTN_GRIPL2 = 0x130, 0x225, 0x226
        gamepad_axes = {0: -20000, 1: -12000, 3: 3000, 4: 3000, 16: 1234, 17: -2345, 18: 0, 19: 0, 20: 2000, 21: 1000}
        motion_axes = {0: 1000, 1: 16384, 2: 2000, 3: 16, 4: 48, 5: 32}
        def evdev_state(node):
            keys, axes = set(), {}
            for line in machine.succeed(f"ctrlassist-fixture evdev-state {node}").splitlines():
                kind, code, *value = line.split()
                if kind == "key":
                    keys.add(int(code))
                else:
                    axes[int(code)] = int(value[0])
            return keys, axes

        # The machine's lizard_mode is written here, which no test on a
        # desktop may do: off, the driver turns lizard mode off as it binds
        # and the gamepad speaks at once; on, its default, the gamepad is
        # silent until the menu button is held.
        for lizard in ["N", "Y"]:
            unit = f"busy-{lizard}"
            machine.succeed(f"echo {lizard} > /sys/module/hid_steam/parameters/lizard_mode")
            menu = " --menu-first" if lizard == "Y" else ""
            machine.succeed(f"systemd-run --unit {unit} ctrlassist-fixture fake-steam --busy --pulse 20{menu}")
            machine.wait_until_succeeds(f"journalctl -u {unit} -o cat | grep -c '^sent ' | grep -qv '^0$'")
            log = steam_log(unit)
            if lizard == "N":
                assert re.search(r"^received set-feature 0181", log, re.M), log
                assert re.search(r"^received set-feature 0187060900004700", log, re.M), log
            else:
                assert re.search(r"^received set-feature 0185", log, re.M), log
                assert "silent true" in log.splitlines(), log
                assert "gamepad-mode" in log.splitlines(), log
            keys, axes = evdev_state(said("gamepad", unit))
            assert keys == {BTN_SOUTH, BTN_GRIPR, BTN_GRIPL2}, (lizard, keys)
            assert all(axes[code] == value for code, value in gamepad_axes.items()), (lizard, axes)
            _, axes = evdev_state(said("motion", unit))
            assert all(axes[code] == value for code, value in motion_axes.items()), (lizard, axes)
            machine.succeed(f"systemctl stop {unit}")
        machine.succeed("cat /sys/module/hid_steam/parameters/lizard_mode | grep -x Y")

    with subtest("a fake Steam Controller with lizard mode's mouse and keyboard has them in the gamepad's HID device"):
        machine.succeed("systemd-run --unit lizard ctrlassist-fixture fake-steam --lizard-mouse-and-keyboard")
        machine.wait_until_succeeds("journalctl -u lizard -o cat | grep '^raw '")
        phys, gamepad = said("phys", "lizard"), said("gamepad", "lizard")
        machine.succeed("udevadm settle")
        inputs = machine.succeed(f"grep -lx '{phys}' /sys/class/input/input*/phys | xargs -n1 dirname").split()
        assert len(inputs) == 3, inputs
        hid = machine.succeed(f"readlink -f /sys/class/input/{gamepad.split('/')[-1]}/device/device").strip()
        keyboards = []
        for input in inputs:
            assert machine.succeed(f"readlink -f {input}/device").strip() == hid, (input, hid)
            event = machine.succeed(f"ls -d {input}/event* | xargs -n1 basename").strip()
            if "ID_INPUT_KEYBOARD=1" in machine.succeed(f"udevadm info /dev/input/{event}"):
                keyboards.append(event)
                machine.succeed(f"udevadm info /dev/input/{event} | grep -x 'E: ID_INPUT_MOUSE=1'")
        assert len(keyboards) == 1, keyboards
        machine.succeed("systemctl stop lizard")

        # Outside a virtual machine check the fixture refuses to make one.
        machine.succeed("mv /etc/ctrlassist-vm-check /etc/ctrlassist-vm-check.aside")
        out = machine.fail("ctrlassist-fixture fake-steam --lizard-mouse-and-keyboard 2>&1")
        assert "only in a virtual machine check" in out, out
        machine.succeed("mv /etc/ctrlassist-vm-check.aside /etc/ctrlassist-vm-check")

    with subtest("a seat user's input thread merges two pads and sleeps when they do"):
        # Two pads that send nothing unless told to.
        machine.succeed("systemd-run --unit quiet-a ctrlassist-fixture fake-pad --name 'Quiet A'")
        machine.succeed("systemd-run --unit quiet-b ctrlassist-fixture fake-pad --name 'Quiet B' --pulse 50")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Quiet' | grep -x 2")
        machine.succeed("udevadm settle")
        a = machine.succeed("ctrlassist-fixture pads | grep 'Quiet A' | grep -o 'event=[^: ]*' | cut -d= -f2").strip()
        b = machine.succeed("ctrlassist-fixture pads | grep 'Quiet B' | grep -o 'event=[^: ]*' | cut -d= -f2").strip()
        machine.succeed(f"systemd-run --unit mux --uid alice ctrlassist-fixture mux {a} {b}")
        machine.wait_until_succeeds("journalctl -u mux -o cat | grep '^realtime='")
        virtual = machine.succeed("journalctl -u mux -o cat | grep -o 'node=.*' | cut -d= -f2").strip()
        machine.succeed("udevadm settle")
        # The assist's presses come out of the virtual pad.
        out = machine.succeed(f"su -l alice -c 'ctrlassist-fixture read {virtual} --frames 4'")
        presses = [line.split()[1] for line in out.splitlines() if line.startswith("frame ")]
        assert len(presses) == 4 and all(x != y for x, y in zip(presses, presses[1:])), out

        # With both pads silent, the input thread is never scheduled.
        machine.succeed("systemctl stop quiet-b")
        machine.wait_until_succeeds("journalctl -u mux -o cat | grep 'report PadGone'")
        switches = (
            "pid=$(systemctl show mux -p MainPID --value); "
            "for t in /proc/$pid/task/*; do "
            "if [ \"$(cat $t/comm)\" = input ]; then grep ctxt_switches $t/status | awk '{s+=$2} END {print s}'; fi; done"
        )
        before = machine.succeed(switches).strip()
        machine.sleep(3)
        after = machine.succeed(switches).strip()
        assert before and before == after, f"the idle input thread was scheduled: {before} then {after}"

    with subtest("a pad that is unplugged and plugged in again is read again"):
        machine.succeed("systemd-run --unit quiet-b2 ctrlassist-fixture fake-pad --name 'Quiet B' --pulse 50")
        machine.wait_until_succeeds("journalctl -u mux -o cat | grep 'pad back source=1'")
        out = machine.succeed(f"su -l alice -c 'ctrlassist-fixture read {virtual} --frames 4'")
        presses = [line.split()[1] for line in out.splitlines() if line.startswith("frame ")]
        assert len(presses) == 4 and all(x != y for x, y in zip(presses, presses[1:])), out
        machine.succeed("systemctl stop mux quiet-a quiet-b2")

    with subtest("a pad that is unplugged is reported as gone"):
        machine.succeed(f"systemd-run --unit reader ctrlassist-fixture read {node} --frames 1000000")
        machine.wait_until_succeeds("journalctl -u reader | grep -c 'frame '")
        machine.succeed("systemctl stop pad")
        machine.wait_until_fails("systemctl is-active reader")
        assert "the pad has gone" in machine.succeed("journalctl -u reader -o cat"), "the reader didn't say the pad had gone"
  '';
}
