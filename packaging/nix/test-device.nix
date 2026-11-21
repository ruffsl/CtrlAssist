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

    with subtest("a pad that is unplugged is reported as gone"):
        machine.succeed(f"systemd-run --unit reader ctrlassist-fixture read {node} --frames 1000000")
        machine.wait_until_succeeds("journalctl -u reader | grep -c 'frame '")
        machine.succeed("systemctl stop pad")
        machine.wait_until_fails("systemctl is-active reader")
        assert "the pad has gone" in machine.succeed("journalctl -u reader -o cat"), "the reader didn't say the pad had gone"
  '';
}
