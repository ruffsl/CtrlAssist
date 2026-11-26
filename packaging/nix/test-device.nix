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
    boot.kernelModules = [
      "uinput"
      "uhid"
      "hid-playstation"
      "joydev"
    ];
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
