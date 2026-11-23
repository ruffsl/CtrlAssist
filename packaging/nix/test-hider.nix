# The hiding service, end to end, on a clean machine with one user at a
# graphical session: hide a pad, lose and regain it, show it again, and
# every way of ending that isn't a polite restore. The service runs as it
# is installed: its own user, no capabilities, the shipped rule.
{
  pkgs,
  fixture,
  hider,
}:
let
  # A wired Xbox 360 pad on the USB bus, for the xpad driver: a USB gadget
  # made through configfs and FunctionFS, plugged into the dummy host
  # controller. It enumerates and holds; it sends no input.
  xpad = pkgs.writers.writePython3Bin "xpad-gadget" { doCheck = false; } ''
    import os
    import struct
    import subprocess
    import sys

    GADGET = "/sys/kernel/config/usb_gadget/xpad"
    FFS = "/dev/ffs-xpad"


    def write(path, value):
        with open(path, "w") as file:
            file.write(value)


    def descriptors():
        # One vendor-specific interface, which is what xpad matches an
        # Xbox 360 pad by, with an interrupt endpoint each way.
        interface = struct.pack("<BBBBBBBBB", 9, 4, 0, 0, 2, 0xFF, 0x5D, 0x01, 1)

        def endpoint(address, interval):
            return struct.pack("<BBBBHB", 7, 5, address, 3, 32, interval)

        full = interface + endpoint(0x81, 4) + endpoint(0x02, 8)
        high = interface + endpoint(0x81, 4) + endpoint(0x02, 4)
        body = struct.pack("<III", 1 | 2, 3, 3) + full + high
        # FUNCTIONFS_DESCRIPTORS_MAGIC_V2, then the length of the whole.
        return struct.pack("<II", 3, 8 + len(body)) + body


    def strings():
        text = b"xpad\0"
        body = struct.pack("<IIH", 1, 1, 0x0409) + text
        return struct.pack("<II", 2, 8 + len(body)) + body


    def main(serial):
        for module in ["dummy_hcd", "libcomposite", "usb_f_fs", "xpad"]:
            subprocess.run(["${pkgs.kmod}/bin/modprobe", module], check=True)
        os.makedirs(GADGET, exist_ok=True)
        for name, value in [
            ("idVendor", "0x045e"),
            ("idProduct", "0x028e"),
            ("bcdDevice", "0x0114"),
            ("bcdUSB", "0x0200"),
            ("bDeviceClass", "0xff"),
            ("bDeviceSubClass", "0xff"),
            ("bDeviceProtocol", "0xff"),
        ]:
            write(f"{GADGET}/{name}", value)
        os.makedirs(f"{GADGET}/strings/0x409", exist_ok=True)
        write(f"{GADGET}/strings/0x409/serialnumber", serial)
        write(f"{GADGET}/strings/0x409/manufacturer", "Fixture")
        write(f"{GADGET}/strings/0x409/product", "Controller")
        os.makedirs(f"{GADGET}/configs/c.1/strings/0x409", exist_ok=True)
        write(f"{GADGET}/configs/c.1/strings/0x409/configuration", "xpad")
        os.makedirs(f"{GADGET}/functions/ffs.xpad", exist_ok=True)
        link = f"{GADGET}/configs/c.1/ffs.xpad"
        if not os.path.exists(link):
            os.symlink(f"{GADGET}/functions/ffs.xpad", link)
        os.makedirs(FFS, exist_ok=True)
        subprocess.run(["${pkgs.util-linux}/bin/mount", "-t", "functionfs", "xpad", FFS], check=True)
        ep0 = os.open(f"{FFS}/ep0", os.O_RDWR)
        os.write(ep0, descriptors())
        os.write(ep0, strings())
        write(f"{GADGET}/UDC", sorted(os.listdir("/sys/class/udc"))[0])
        print("bound", flush=True)
        # Answer the host: an event is twelve bytes, a setup request then
        # its type. A request this function doesn't know is answered empty.
        while True:
            event = os.read(ep0, 12)
            request_type, _, _, _, length, kind = struct.unpack("<BBHHHB3x", event)
            if kind == 4:
                if request_type & 0x80:
                    os.write(ep0, b"")
                else:
                    os.read(ep0, length)


    main(sys.argv[1])
  '';

  # A client of the service that does what the fixture's well-behaved one
  # doesn't, speaking the wire format directly: a request and a token,
  # five bytes; a status, a token and a flag, six.
  #
  #   hider-probe opath <socket> <opcode> <node>
  #       passes a descriptor on <node> opened O_PATH, says the status
  #   hider-probe hide <socket> <node>
  #       hides <node>, says the status and the name of the pad whose
  #       node came back, then closes, which shows the pad again
  #   hider-probe silent <socket> <count>
  #       sends up to <count> requests and never reads a reply; holds
  #   hider-probe idle <socket> <count>
  #       opens up to <count> connections and sends nothing; holds
  probe = pkgs.writers.writePython3Bin "hider-probe" { doCheck = false; } ''
    import array
    import os
    import resource
    import socket
    import struct
    import sys
    import time


    def connect(path):
        peer = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        peer.connect(path)
        return peer


    def ask(peer, opcode, fd=None):
        message = struct.pack("<BI", opcode, 0)
        rights = []
        if fd is not None:
            fds = array.array("i", [fd])
            rights = [(socket.SOL_SOCKET, socket.SCM_RIGHTS, fds)]
        peer.sendmsg([message], rights)
        data, fds, _, _ = socket.recv_fds(peer, 6, 2)
        status, token, _ = struct.unpack("<BIB", data)
        return status, token, fds


    def name_of(fd):
        node = os.readlink(f"/proc/self/fd/{fd}")
        base = os.path.basename(node)
        with open(f"/sys/class/input/{base}/device/name") as file:
            return node, file.read().strip()


    match sys.argv[1:]:
        case ["opath", path, opcode, node]:
            fd = os.open(node, os.O_PATH)
            status, _, fds = ask(connect(path), int(opcode), fd)
            print(f"status={status} fds={len(fds)}")
        case ["hide", path, node]:
            fd = os.open(node, os.O_RDONLY)
            status, token, fds = ask(connect(path), 1, fd)
            if fds:
                node, name = name_of(fds[0])
                print(f"status={status} node={node} name={name}")
            else:
                print(f"status={status}")
        case ["silent", path, count]:
            peer = connect(path)
            peer.setblocking(False)
            sent = 0
            for _ in range(int(count)):
                try:
                    peer.send(struct.pack("<BI", 6, 0))
                except BlockingIOError:
                    break
                sent += 1
            print(f"sent={sent}", flush=True)
            time.sleep(3600)
        case ["idle", path, count]:
            _, most = resource.getrlimit(resource.RLIMIT_NOFILE)
            resource.setrlimit(resource.RLIMIT_NOFILE, (most, most))
            peers = []
            for _ in range(int(count)):
                try:
                    peers.append(connect(path))
                except OSError as error:
                    print(f"stopped: {error}", flush=True)
                    break
            print(f"connected={len(peers)}", flush=True)
            time.sleep(3600)
        case _:
            sys.exit("usage: hider-probe opath|hide|silent|idle ...")
  '';
in
pkgs.testers.runNixOSTest {
  name = "ctrlassist-hider";

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
      # In the input group as well, as some distributions and guides
      # have gamers be: hiding must close a pad to her all the same.
      extraGroups = [ "input" ];
    };
    # Another local user, not at the seat.
    users.users.bob = {
      isNormalUser = true;
      uid = 1001;
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

    # The units and the rule as packaged, hardening included, and the
    # user the unit runs as, as the module makes it.
    systemd.packages = [ hider ];
    systemd.sockets.ctrlassist-hider.wantedBy = [ "sockets.target" ];
    users.users.ctrlassist-hider = {
      isSystemUser = true;
      group = "ctrlassist-hider";
    };
    users.groups.ctrlassist-hider = { };

    services.udev.packages = [
      hider
      # What Steam's 60-steam-input.rules does for a DualSense: opens its
      # hidraw node to the seat user, by its HID device's name, which
      # covers USB and Bluetooth alike. Hiding must close it all the same.
      (pkgs.writeTextDir "lib/udev/rules.d/60-steam-input.rules" ''
        KERNEL=="hidraw*", KERNELS=="*054C:0CE6*", MODE="0660", TAG+="uaccess"
      '')
      # A pad on another seat, as a pad plugged into a second seat's hub
      # is: by its name, the fake pad of that name is put on seat1.
      (pkgs.writeTextDir "lib/udev/rules.d/61-test-seat.rules" ''
        SUBSYSTEM=="input", ATTRS{name}=="Seat One Pad", ENV{ID_SEAT}="seat1"
      '')
    ];

    environment.systemPackages = [
      fixture
      hider
      probe
      xpad
    ];
  };

  testScript = ''
    import re
    import shlex

    SOCKET = "/run/ctrlassist/hider.sock"
    MARKERS = "/run/ctrlassist/hide"

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

    def markers():
        return machine.succeed(f"cd {MARKERS} && find . -type f | sort").split()

    def no_runtime_rules():
        """The service writes no udev rule: the directory for runtime rules never comes to be."""
        machine.succeed("test ! -e /run/udev/rules.d || test -z \"$(ls -A /run/udev/rules.d)\"")

    def wait_restored():
        """Waits until nothing is hidden: no marker left, and udev done."""
        machine.wait_until_succeeds(f"test -z \"$(ls -A {MARKERS})\"")
        machine.succeed("udevadm settle")
        no_runtime_rules()

    def listening(unit, node):
        machine.succeed(f"systemd-run --unit {unit} --uid alice ctrlassist-fixture listen {node} --millis 60000")
        machine.wait_until_succeeds(f"journalctl -u {unit} | grep 't=1s'")

    def cut_off(unit):
        machine.wait_until_fails(f"systemctl is-active {unit}")
        return "cut_off=true" in machine.succeed(f"journalctl -u {unit} -o cat")

    A, B = "02:00:00:00:00:01", "02:00:00:00:00:02"

    machine.wait_for_file("/run/user/1000/wayland-0.lock")
    machine.wait_for_unit("sockets.target")
    print("versions: kernel " + machine.succeed("uname -r").strip() + ", udev " + machine.succeed("udevadm --version").strip())
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

    with subtest("the service won't run as root"):
        out = machine.fail("ctrlassist-hider reconcile 2>&1")
        assert "not as root" in out, out

    with subtest("the service says its release to the user in front, and nobody else"):
        version = f"ctrlassist-fixture version --socket {SOCKET}"
        told = machine.succeed(f"su -l alice -c '{version}'").strip()
        assert told == "version: ${hider.version}", told
        assert "isn't the user in front" in machine.succeed(version)

    with subtest("it runs as its own user, with no capabilities"):
        # A client that holds a pad hidden keeps the service running.
        control, told = hide("holder", pads()[B]["event"][0])
        assert told.startswith("ok"), told
        pid = machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip()
        assert pid != "0", "the service is running for the client"
        status = machine.succeed(f"cat /proc/{pid}/status")
        machine.succeed("systemctl stop holder")
        wait_restored()
        uids = field(r"Uid:\s+([0-9\s]+)\n", status).split()
        gids = field(r"Gid:\s+([0-9\s]+)\n", status).split()
        user = machine.succeed("id -u ctrlassist-hider").strip()
        assert uids == [user] * 4 and "0" not in gids, status
        for capabilities in ("CapInh", "CapPrm", "CapEff", "CapBnd", "CapAmb"):
            assert field(capabilities + r":\s+(\w+)", status) == "0000000000000000", status
        print(f"the service: uid {uids[0]}, " + ", ".join(line for line in status.splitlines() if line.startswith("Cap")))

    with subtest("a raw node is opened only for the kinds of pad the service names"):
        raw = f"ctrlassist-fixture raw {pads()[A]['event'][0]} --socket {SOCKET}"
        told = machine.succeed(f"su -l alice -c '{raw}'")
        assert "can't do that for this kind of pad" in told, told
        assert "isn't the user in front" in machine.succeed(raw)
        # A pad of the PlayStation driver: a virtual DualSense with an
        # address a real one could have.
        fake("pad-ds", "fake-dualsense", "--real")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 02:ca:7e:00:00:01")
        machine.succeed("udevadm settle")
        DS = "02:ca:7e:00:00:01"
        dualsense = pads()[DS]
        assert alice_can_open(DS, "hidraw"), "the Steam-like rule opens its raw node to the seat user"
        raw = f"ctrlassist-fixture raw {dualsense['event'][0]} --socket {SOCKET}"
        assert "raw: ok" in machine.succeed(f"su -l alice -c '{raw}'")
        assert "isn't the user in front" in machine.succeed(raw)
        # Hidden, its raw node is closed to the seat user, though another
        # rule opens it, and still to be had by whoever hid it; a hidden
        # pad of another driver's isn't.
        control, told = hide("hide-ds", dualsense["event"][0])
        assert told.startswith("ok"), told
        machine.succeed("udevadm settle")
        assert not alice_can_open(DS, "event"), "a hidden DualSense's event node"
        assert not alice_can_open(DS, "hidraw"), "a hidden DualSense's raw node, which a 60- rule opens"
        print("hidden DualSense: " + machine.succeed(f"stat -c '%n %a %U:%G' {pads()[DS]['event'][0]} {pads()[DS]['hidraw'][0]}"))
        assert order(control, "raw") == "ok", "the raw node of a hidden pad of the PlayStation driver"
        assert order(control, "restore") == "ok"
        machine.succeed("udevadm settle")
        assert alice_can_open(DS, "event") and alice_can_open(DS, "hidraw"), "a restored DualSense"
        machine.succeed("systemctl stop hide-ds")
        control, told = hide("hide-plain", pads()[A]["event"][0])
        assert told.startswith("ok"), told
        assert "can't do that for this kind of pad" in order(control, "raw")
        assert order(control, "restore") == "ok"
        machine.succeed("systemctl stop hide-plain")
        wait_restored()
        machine.succeed("systemctl stop pad-ds")
        machine.wait_until_fails("ctrlassist-fixture pads | grep -c 02:ca:7e:00:00:01")

    with subtest("someone not in front of the machine is refused"):
        _, told = hide("as-root", pads()[A]["event"][0], user="root")
        assert "isn't the user in front" in told, told
        # The refused client has exited by itself.
        machine.wait_until_fails("systemctl is-active as-root")
        assert alice_can_open(A)

    with subtest("hiding re-binds the driver, closes the pad to the seat user and cuts off whoever had it open"):
        node = pads()[A]["event"][0]
        listening("early", node)
        control, told = hide("client", node)
        assert told.startswith("ok") and "dislodged=true" in told, told
        assert cut_off("early"), "the early reader kept reading"
        assert not alice_can_open(A, "event") and not alice_can_open(A, "js")
        found = markers()
        assert f"./addr-{A}" in found and any(marker.startswith("./hid-") for marker in found), found
        print("markers: " + " ".join(found))
        no_runtime_rules()

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
        no_runtime_rules()

    with subtest("showing it again gives the pad back and leaves nothing behind"):
        assert order(control, "restore").startswith("ok")
        machine.succeed("udevadm settle")
        assert alice_can_open(A, "event") and alice_can_open(A, "js")
        assert markers() == [], markers()
        no_runtime_rules()
        machine.succeed("systemctl stop client")

    with subtest("a client that is killed gets its pads shown again"):
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

    with subtest("a pad's own name and address can't reach a marker's name"):
        evil = 'evil/../../x", RUN+="/bin/touch /tmp/pwned'
        fake("pad-evil", "fake-hid", "--name", evil, "--uniq", evil, "--product", "0bad", "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c '^pad ' | grep -x 3")
        machine.succeed("udevadm settle")
        control, told = hide("hostile", pads()[evil]["event"][0])
        assert told.startswith("ok"), told
        assert not alice_can_open(evil)
        found = markers()
        assert not any("evil" in marker or ".." in marker for marker in found), found
        machine.succeed("test ! -e /tmp/pwned")
        assert order(control, "restore").startswith("ok")
        machine.succeed("systemctl stop hostile pad-evil")
        wait_restored()

    def listing(as_user=None):
        """Each pad the fixture lists, as its name, its address and its nodes: for pads that share an address."""
        command = "ctrlassist-fixture pads"
        if as_user:
            command = f"su -l {as_user} -c '{command}'"
        found = []
        quoted = r'"((?:[^"\\]|\\.)*)"'
        for line in machine.succeed(command).splitlines():
            nodes = {kind: tuple(node.rsplit(":", 1)) for kind, node in re.findall(r"(event|js|hidraw)=(\S+)", line)}
            found.append((field("name=" + quoted, line), field("uniq=" + quoted, line), nodes))
        return found

    def one(name, uniq, as_user=None):
        return [nodes for named, address, nodes in listing(as_user) if named == name and address == uniq][0]

    with subtest("a descriptor opened O_PATH names no pad"):
        node = pads()[A]["event"][0]
        # Hide, then raw: each refused as no pad, with nothing passed back.
        for opcode in (1, 4):
            told = machine.succeed(f"su -l alice -c 'hider-probe opath {SOCKET} {opcode} {node}'")
            assert "status=2 fds=0" in told, told
        assert alice_can_open(A)
        wait_restored()

    with subtest("a forged twin with another pad's numbers and address doesn't get the real pad"):
        fake("pad-twin", "fake-hid", "--name", "Twin Pad", "--uniq", A, "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Twin Pad'")
        machine.succeed("udevadm settle")
        real = one("Fixture Fake HID Pad", A)["event"][0]
        twin = one("Twin Pad", A)["event"][0]
        told = machine.succeed(f"su -l alice -c 'hider-probe hide {SOCKET} {twin}'")
        print(f"hiding the twin {twin} beside the real pad {real}: {told.strip()}")
        assert "status=0" in told and "name=Twin Pad" in told, told
        assert field(r"node=(\S+)", told) != real, told
        wait_restored()
        assert one("Fixture Fake HID Pad", A, as_user="alice")["event"] == (real, "open"), "the real pad untouched"
        # The real pad can still be hidden, and is.
        control, told = hide("real", real)
        assert told.startswith("ok"), told
        assert one("Fixture Fake HID Pad", A, as_user="alice")["event"][1] == "denied"
        assert order(control, "restore").startswith("ok")
        machine.succeed("systemctl stop real pad-twin")
        wait_restored()

    with subtest("a pad on another seat is refused to the user in front on this one"):
        fake("pad-seat1", "fake-hid", "--name", "Seat One Pad", "--uniq", "02:00:00:00:01:01", "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Seat One Pad'")
        machine.succeed("udevadm settle")
        node = pads()["02:00:00:00:01:01"]["event"][0]
        assert machine.succeed(f"udevadm info -q property -n {node} | grep ^ID_SEAT=").strip() == "ID_SEAT=seat1"
        # Alice can open it, by the input group, and still may not hide it.
        _, told = hide("seat1", node)
        assert "isn't the user in front" in told, told
        assert markers() == [], markers()
        machine.succeed("systemctl stop pad-seat1")

    with subtest("clients that never read their replies, and many idle connections, don't stop the service serving others"):
        # A client of the seat user's that holds a pad, so the service
        # stays up and its process can be looked at.
        control, told = hide("keeper", pads()[B]["event"][0])
        assert told.startswith("ok"), told
        pid = machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip()
        # Any local user may connect: root isn't at the seat, so it is
        # only refused, again and again, as fast as it asks.
        machine.succeed(f"systemd-run --unit silent-root hider-probe silent {SOCKET} 100000")
        machine.succeed(f"systemd-run --unit silent-alice --uid alice hider-probe silent {SOCKET} 100000")
        machine.succeed(f"systemd-run --unit idle-bob --uid bob hider-probe idle {SOCKET} 3000")
        for unit in ("silent-root", "silent-alice"):
            machine.wait_until_succeeds(f"journalctl -u {unit} -o cat | grep -q sent=")
        machine.wait_until_succeeds("journalctl -u idle-bob -o cat | grep -q connected=")
        print("probes: " + machine.succeed("journalctl -u silent-root -u silent-alice -u idle-bob -o cat | grep -E 'sent=|connected=|stopped'").replace("\n", " "))
        # Served all the same: its release told, a pad hidden and shown again.
        told = machine.succeed(f"su -l alice -c 'ctrlassist-fixture version --socket {SOCKET}'").strip()
        assert told == "version: ${hider.version}", told
        other, told = hide("served", pads()[A]["event"][0])
        assert told.startswith("ok"), told
        assert not alice_can_open(A)
        assert order(other, "restore").startswith("ok")
        assert alice_can_open(A)
        machine.succeed("systemctl stop served")
        assert machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip() == pid, "the service didn't restart"
        print("the service holds " + machine.succeed(f"ls /proc/{pid}/fd | wc -l").strip() + " descriptors and " + machine.succeed(f"ls /proc/{pid}/task | wc -l").strip() + " threads")
        assert int(machine.succeed(f"ls /proc/{pid}/fd | wc -l")) < 100
        machine.succeed("systemctl stop silent-root silent-alice idle-bob")
        assert order(control, "restore").startswith("ok")
        machine.succeed("systemctl stop keeper")
        wait_restored()

    with subtest("a pad with no driver to re-bind is refused, for the daemon to grab"):
        fake("pad-plain", "fake-pad", "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Fixture Fake Pad'")
        machine.succeed("udevadm settle")
        plain = "Fixture Fake Pad"
        _, told = hide("plain", pads()[plain]["event"][0])
        assert "can't do that for this kind of pad" in told, told
        assert alice_can_open(plain, "event")
        assert markers() == [], markers()
        machine.succeed("systemctl stop pad-plain")

    with subtest("an Xbox 360 pad on USB under xpad is hidden and shown again by re-binding its USB interface"):
        machine.succeed("systemd-run --unit xpad-gadget xpad-gadget XPADFIXTURE1")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c X-Box", timeout=60)
        machine.succeed("udevadm settle")
        key = [key for key in pads() if "X-Box" in key][0]
        node = pads()[key]["event"][0]
        sysname = node.split("/")[-1]
        print("xpad pad: " + machine.succeed(
            f"udevadm info -q property -n {node} | grep -E '^(ID_SERIAL_SHORT|ID_BUS|CURRENT_TAGS)='"
        ).replace("\n", " ") + " driver=" + machine.succeed(f"basename $(readlink -f /sys/class/input/{sysname}/device/device/driver)").strip())
        assert alice_can_open(key, "event")
        listening("early-xpad", node)
        control, told = hide("xpad", node)
        assert told.startswith("ok") and "dislodged=true" in told, told
        assert cut_off("early-xpad"), "the early reader kept reading"
        assert not alice_can_open(key, "event") and not alice_can_open(key, "js")
        assert "./serial-045e-028e-XPADFIXTURE1" in markers(), markers()
        assert order(control, "restore").startswith("ok")
        machine.succeed("udevadm settle")
        assert alice_can_open(key, "event") and alice_can_open(key, "js")
        machine.succeed("systemctl stop xpad")
        wait_restored()
  '';
}
