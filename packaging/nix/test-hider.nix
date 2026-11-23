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
  # A pad on the USB bus, as a USB gadget made through configfs and
  # plugged into one of the dummy host controller's ports, `dummy_udc.<n>`.
  # It enumerates and holds; it sends no input.
  #
  #   usb-gadget xpad <name> <serial> <n>
  #       a wired Xbox 360 pad, for the xpad driver, through FunctionFS
  #   usb-gadget hid <name> <serial> <n> <product> <product string>
  #       a HID game pad, for usbhid and the generic HID driver
  #   usb-gadget kbd <name> <n> <product>
  #       a HID boot keyboard, with no serial
  #
  # A serial of "-" gives the pad none.
  gadget = pkgs.writers.writePython3Bin "usb-gadget" { doCheck = false; } ''
    import os
    import struct
    import subprocess
    import sys
    import time

    # A generic HID game pad: eight buttons, then X and Y as signed bytes,
    # as the test crate's uhid pad has.
    GAME_PAD = bytes([
        0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0xA1, 0x00, 0x05, 0x09,
        0x19, 0x01, 0x29, 0x08, 0x15, 0x00, 0x25, 0x01, 0x95, 0x08,
        0x75, 0x01, 0x81, 0x02, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31,
        0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x02,
        0xC0, 0xC0,
    ])

    # A boot keyboard: eight modifier bits, a reserved byte, six keys, and
    # five lights out.
    KEYBOARD = bytes([
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0,
        0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08,
        0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05,
        0x75, 0x01, 0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02,
        0x95, 0x01, 0x75, 0x03, 0x91, 0x01, 0x95, 0x06, 0x75, 0x08,
        0x15, 0x00, 0x25, 0x65, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65,
        0x81, 0x00, 0xC0,
    ])


    def write(path, value):
        with open(path, "wb" if isinstance(value, bytes) else "w") as file:
            file.write(value)


    def made(name, serial, vendor, product, device_class, product_string, function):
        """The gadget's configfs directory, with one configuration holding `function`."""
        for module in ["dummy_hcd", "libcomposite", "usb_f_fs", "usb_f_hid", "xpad"]:
            subprocess.run(["${pkgs.kmod}/bin/modprobe", module], check=True)
        gadget = f"/sys/kernel/config/usb_gadget/{name}"
        os.makedirs(gadget, exist_ok=True)
        for attribute, value in [
            ("idVendor", vendor),
            ("idProduct", product),
            ("bcdDevice", "0x0114"),
            ("bcdUSB", "0x0200"),
            ("bDeviceClass", device_class),
            ("bDeviceSubClass", device_class),
            ("bDeviceProtocol", device_class),
        ]:
            write(f"{gadget}/{attribute}", value)
        os.makedirs(f"{gadget}/strings/0x409", exist_ok=True)
        if serial != "-":
            write(f"{gadget}/strings/0x409/serialnumber", serial)
        write(f"{gadget}/strings/0x409/manufacturer", "Fixture")
        write(f"{gadget}/strings/0x409/product", product_string)
        os.makedirs(f"{gadget}/configs/c.1/strings/0x409", exist_ok=True)
        write(f"{gadget}/configs/c.1/strings/0x409/configuration", name)
        os.makedirs(f"{gadget}/functions/{function}", exist_ok=True)
        return gadget


    def link(gadget, function, udc):
        """Puts the function in the configuration and plugs the gadget in."""
        configured = f"{gadget}/configs/c.1/{function}"
        if not os.path.exists(configured):
            os.symlink(f"{gadget}/functions/{function}", configured)
        write(f"{gadget}/UDC", f"dummy_udc.{udc}")
        print("bound", flush=True)


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


    def xpad(name, serial, udc):
        function = f"ffs.{name}"
        gadget = made(name, serial, "0x045e", "0x028e", "0xff", "Controller", function)
        ffs = f"/dev/ffs-{name}"
        os.makedirs(ffs, exist_ok=True)
        subprocess.run(["${pkgs.util-linux}/bin/mount", "-t", "functionfs", name, ffs], check=True)
        ep0 = os.open(f"{ffs}/ep0", os.O_RDWR)
        os.write(ep0, descriptors())
        os.write(ep0, strings())
        link(gadget, function, udc)
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


    def hid(name, serial, udc, product, product_string, report=GAME_PAD, kind=("0", "0", "3")):
        function = f"hid.{name}"
        gadget = made(name, serial, "0x1209", product, "0x00", product_string, function)
        protocol, subclass, length = kind
        for attribute, value in [
            ("protocol", protocol),
            ("subclass", subclass),
            ("report_length", length),
            ("report_desc", report),
        ]:
            write(f"{gadget}/functions/{function}/{attribute}", value)
        link(gadget, function, udc)
        while True:
            time.sleep(3600)


    match sys.argv[1:]:
        case ["xpad", name, serial, udc]:
            xpad(name, serial, udc)
        case ["hid", name, serial, udc, product, product_string]:
            hid(name, serial, udc, product, product_string)
        case ["kbd", name, udc, product]:
            hid(name, "-", udc, product, "Keyboard", KEYBOARD, ("1", "1", "8"))
        case _:
            sys.exit("usage: usb-gadget xpad|hid|kbd <name> ...")
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
  #   hider-probe long <socket>
  #       sends a request with bytes after it, says the status
  #   hider-probe silent <socket> <count>
  #       sends up to <count> requests and never reads a reply; holds
  #   hider-probe idle <socket> <count>
  #       opens up to <count> connections and sends nothing; holds
  #   hider-probe uids <socket> <first> <count> <each> [hold]
  #       as each of <count> user IDs from <first>, as a user's
  #       subordinate IDs would, opens <each> connections and asks each
  #       the release; says how many were answered and how many closed at
  #       once; with hold, holds them
  #   hider-probe uhid <name> <phys>
  #       makes a HID game pad through uhid with that phys and no address;
  #       holds
  #   hider-probe cancel <socket> <node> at-once|after|ahead|<fifo>
  #       hides <node> and gives the hide up with a cancel: sent at once,
  #       after the hide's reply, at once behind a version request, or once
  #       something is written to <fifo>; says each reply's status and the
  #       re-binds it counted, and their total
  #   hider-probe cancel-then-hide <socket> <node> <HID device> <fifo>
  #       hides <node>, cancels once something is written to <fifo>, then
  #       hides the pad again; says whether that descriptor still reads
  #   hider-probe uevent <text>
  #       ends once the kernel sends an event with <text> in it
  #   hider-probe rebinds <count>
  #       says each HID device's re-bind request, unbind and bind, as the
  #       kernel sends it (K) and as udev answers it (U), in the order the
  #       two were sent; ends at the <count>th answer to a request
  probe = pkgs.writers.writePython3Bin "hider-probe" { doCheck = false; } ''
    import array
    import glob
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
        case ["long", path]:
            # A version request with more after it.
            peer = connect(path)
            peer.send(struct.pack("<BI", 6, 0) + b"more")
            data, _, _, _ = socket.recv_fds(peer, 6, 2)
            # Bit 1 of the flags says the re-binds are counted.
            print(f"status={data[0]} counted={data[5] >> 1 & 1}")
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
        case ["uids", path, first, count, each, *hold]:
            reader, writer = os.pipe()
            for uid in range(int(first), int(first) + int(count)):
                if os.fork() == 0:
                    os.close(reader)
                    os.setgroups([])
                    os.setgid(uid)
                    os.setuid(uid)
                    peers, closed = [], 0
                    for _ in range(int(each)):
                        try:
                            peer = connect(path)
                            peer.send(struct.pack("<BI", 6, 0))
                            if peer.recv(6):
                                peers.append(peer)
                            else:
                                closed += 1
                        except OSError:
                            closed += 1
                    os.write(writer, f"{len(peers)} {closed}\n".encode())
                    if hold:
                        time.sleep(3600)
                    os._exit(0)
            os.close(writer)
            answered = closed = 0
            with os.fdopen(reader) as lines:
                for _ in range(int(count)):
                    one, other = lines.readline().split()
                    answered += int(one)
                    closed += int(other)
            print(f"answered={answered} closed={closed}", flush=True)
            if hold:
                time.sleep(3600)
        case ["uhid", name, phys]:
            # The test crate's uhid pad's descriptor: a game pad.
            descriptor = bytes([
                0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0xA1, 0x00, 0x05, 0x09,
                0x19, 0x01, 0x29, 0x08, 0x15, 0x00, 0x25, 0x01, 0x95, 0x08,
                0x75, 0x01, 0x81, 0x02, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31,
                0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x02,
                0xC0, 0xC0,
            ])
            uhid = os.open("/dev/uhid", os.O_RDWR)
            # UHID_CREATE2: name, phys, uniq, the descriptor's size, the
            # bus (USB), vendor, product, version, country, the descriptor.
            os.write(uhid, struct.pack(
                "<I128s64s64sHHIIII4096s",
                11, name.encode(), phys.encode(), b"", len(descriptor),
                3, 0x1209, 0x0bae, 1, 0, descriptor,
            ))
            print("created", flush=True)
            while True:
                os.read(uhid, 4380)
        case ["cancel", path, node, when]:
            peer = connect(path)
            fd = os.open(node, os.O_RDONLY)
            fds = array.array("i", [fd])
            rights = [(socket.SOL_SOCKET, socket.SCM_RIGHTS, fds)]
            peer.sendmsg([struct.pack("<BI", 1, 0)], rights)
            os.close(fd)

            def reply():
                data, fds, _, _ = socket.recv_fds(peer, 6, 2)
                for received in fds:
                    os.close(received)
                status, _, flags = struct.unpack("<BIB", data)
                # Bit 1 says the re-binds are counted, bits 2 and 3 how many.
                assert flags & 2, f"not counted: {flags}"
                return status, flags >> 2 & 3

            replies = []
            if when == "after":
                replies.append(reply())
            elif when == "ahead":
                # A request sent as the hide is under way, ahead of the
                # cancel: it doesn't give the hide up, and is answered in
                # its turn, after the hide.
                peer.send(struct.pack("<BI", 6, 0))
            elif when != "at-once":
                with open(when) as go:
                    go.read()
            peer.send(struct.pack("<BI", 7, 0))
            while len(replies) < (3 if when == "ahead" else 2):
                replies.append(reply())
            if when == "ahead":
                (hide, hidden), (version, _), (cancel, cancelled) = replies
                print(
                    f"hide={hide} rebinds={hidden} version={version} cancel={cancel} "
                    f"rebinds={cancelled} total={hidden + cancelled}",
                    flush=True,
                )
            else:
                (hide, hidden), (cancel, cancelled) = replies
                print(
                    f"hide={hide} rebinds={hidden} cancel={cancel} rebinds={cancelled} "
                    f"total={hidden + cancelled}",
                    flush=True,
                )
        case ["cancel-then-hide", path, node, hid, go]:
            # A hide given up once something is written to <go>, then, its
            # replies read, the pad hidden again on its node as it is now;
            # says whether the descriptor that hide passed back still reads.
            peer = connect(path)
            fd = os.open(node, os.O_RDONLY)
            peer.sendmsg([struct.pack("<BI", 1, 0)], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", [fd]))])
            os.close(fd)
            with open(go) as waited:
                waited.read()
            peer.send(struct.pack("<BI", 7, 0))
            print("cancelled", flush=True)
            told = []
            for _ in range(2):
                data, fds, _, _ = socket.recv_fds(peer, 6, 2)
                status, _, flags = struct.unpack("<BIB", data)
                told.append(f"{status} rebinds={flags >> 2 & 3}")
            [event] = glob.glob(f"/sys/bus/hid/devices/{hid}/input/input*/event*")
            fd = os.open("/dev/input/" + os.path.basename(event), os.O_RDONLY)
            peer.sendmsg([struct.pack("<BI", 1, 0)], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", [fd]))])
            os.close(fd)
            data, fds, _, _ = socket.recv_fds(peer, 6, 2)
            status, _, flags = struct.unpack("<BIB", data)
            # The pad reports every 20 ms: a live descriptor has something
            # to read by now, and one the pad's re-bind cut off fails.
            time.sleep(1)
            try:
                live = len(os.read(fds[0], 4096)) > 0
            except BlockingIOError:
                live = True
            except OSError:
                live = False
            print(
                f"hide={told[0]} cancel={told[1]} again={status} rebinds={flags >> 2 & 3} live={live}",
                flush=True,
            )
        case ["uevent", match]:
            # Says "seen" and ends once the kernel sends an event with
            # <match> in it, before udev has taken it.
            events = socket.socket(socket.AF_NETLINK, socket.SOCK_DGRAM, 15)
            events.bind((0, 1))
            print("listening", flush=True)
            while match.encode() not in events.recv(65536):
                pass
            print("seen", flush=True)
        case ["rebinds", count]:
            # The kernel's group and udev's on one socket, so what each
            # sends is queued here in the order it was sent.
            events = socket.socket(socket.AF_NETLINK, socket.SOCK_DGRAM, 15)
            events.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1 << 20)
            events.bind((0, 1 | 2))
            print("listening", flush=True)
            answered = 0
            while answered < int(count):
                data = events.recv(65536)
                if data.startswith(b"libudev\0"):
                    # udev's header: its properties' offset and length.
                    offset, length = struct.unpack_from("=II", data, 16)
                    source, fields = "U", data[offset:offset + length].split(b"\0")
                else:
                    source, fields = "K", data.split(b"\0")[1:]
                found = dict(field.decode(errors="replace").split("=", 1) for field in fields if b"=" in field)
                if found.get("SUBSYSTEM") != "hid":
                    continue
                action = found.get("ACTION")
                if action == "change" and found.get("SYNTH_ARG_CTRLASSIST") == "rebind":
                    print(f"{source} change {found['SYNTH_UUID']}", flush=True)
                    answered += source == "U"
                elif action in ("unbind", "bind"):
                    print(f"{source} {action}", flush=True)
        case _:
            sys.exit("usage: hider-probe opath|hide|long|silent|idle|uids|uhid|cancel|cancel-then-hide|uevent|rebinds ...")
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
      # The HID pads made as USB gadgets have their raw nodes opened to
      # the seat user the same way, so that hiding them shows.
      (pkgs.writeTextDir "lib/udev/rules.d/60-test-hidraw.rules" ''
        KERNEL=="hidraw*", KERNELS=="*1209:5EF?.*", MODE="0660", TAG+="uaccess"
      '')
      # A later rule that gives seat access back to the pad of that name,
      # which the shipped rule's final assignments can't stop.
      (pkgs.writeTextDir "lib/udev/rules.d/99-test-retag.rules" ''
        SUBSYSTEM=="input", ATTRS{name}=="Retagged Pad", TAG+="uaccess"
      '')
      # Holds a re-bind of the pad of that name while /run/hold is a FIFO
      # nobody has written to: udev runs this for each event node the
      # re-bind makes and isn't done with the node until it ends, so the
      # service waits too. With no FIFO there, it ends at once.
      (pkgs.writeTextDir "lib/udev/rules.d/99-test-hold.rules" ''
        ACTION=="add", SUBSYSTEM=="input", KERNEL=="event*", ATTRS{name}=="Held Pad", RUN+="${pkgs.coreutils}/bin/cat /run/hold"
      '')
      # Holds udev's queue for a HID device before anything after it: a
      # change event written to its uevent file with TESTHOLD=1, which
      # udev isn't done with until /run/hold-early is written to, so every
      # later event of the device, a re-bind request among them, waits.
      (pkgs.writeTextDir "lib/udev/rules.d/99-test-hold-early.rules" ''
        ACTION=="change", SUBSYSTEM=="hid", ENV{SYNTH_ARG_TESTHOLD}=="1", RUN+="${pkgs.coreutils}/bin/cat /run/hold-early"
      '')
    ];
    # Four ports, each its own bus, for the pads made as USB gadgets and
    # what is plugged in after them.
    boot.extraModprobeConfig = "options dummy_hcd num=4";

    environment.systemPackages = [
      fixture
      gadget
      hider
      probe
      pkgs.acl
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

    def alice_opens(node):
        """Whether alice can open a node for reading, by whatever grants it."""
        return machine.execute(f"su -l alice -c 'exec 3< {node}'")[0] == 0

    def other_nodes(event):
        """Every event, js and mouse node under the device a pad's event node hangs from, but those of the pad's own input."""
        name = event.split("/")[-1]
        own = machine.succeed(f"basename $(readlink -f /sys/class/input/{name}/device)").strip()
        device = machine.succeed(f"readlink -f /sys/class/input/{name}/device/device").strip()
        found = machine.succeed(f"cd {device}/input && ls -d */event* */js* */mouse* 2>/dev/null || true").split()
        return sorted("/dev/input/" + path.split("/")[1] for path in found if path.split("/")[0] != own)

    def sysfs(node, path):
        """A path under the sysfs directory of a node's input device, resolved."""
        name = node.split("/")[-1]
        return machine.succeed(f"readlink -f /sys/class/input/{name}/device/{path}").strip()

    def replug(gadget, udc, gone):
        """Unplugs a USB gadget from its port of the dummy host controller and plugs it in again, as a pad taken out and put back in the same port."""
        machine.succeed(f"echo > /sys/kernel/config/usb_gadget/{gadget}/UDC")
        machine.wait_until_fails(f"ctrlassist-fixture pads | grep -F {shlex.quote(gone)}")
        machine.succeed(f"echo dummy_udc.{udc} > /sys/kernel/config/usb_gadget/{gadget}/UDC")
        machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -F {shlex.quote(gone)}", timeout=60)
        machine.succeed("udevadm settle")

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
        # Its touchpad and motion sensors, and the touchpad's mouse node,
        # are open to her through the input group.
        others = other_nodes(dualsense["event"][0])
        assert len(others) >= 2, f"a DualSense has a touchpad and motion sensors: {others}"
        assert all(alice_opens(node) for node in others), others
        # Hidden, its raw node is closed to the seat user, though another
        # rule opens it, and still to be had by whoever hid it; a hidden
        # pad of another driver's isn't.
        control, told = hide("hide-ds", dualsense["event"][0])
        assert told.startswith("ok"), told
        machine.succeed("udevadm settle")
        assert not alice_can_open(DS, "event"), "a hidden DualSense's event node"
        assert not alice_can_open(DS, "hidraw"), "a hidden DualSense's raw node, which a 60- rule opens"
        print("hidden DualSense: " + machine.succeed(f"stat -c '%n %a %U:%G' {pads()[DS]['event'][0]} {pads()[DS]['hidraw'][0]}"))
        # And every other node of it, as every node of a hidden pad is.
        others = other_nodes(pads()[DS]["event"][0])
        for node in others:
            print("hidden DualSense's other node: " + machine.succeed(f"stat -c '%n %a %U:%G' {node}").strip() + " " + machine.succeed(f"udevadm info -q property -n {node} | grep -E '^ID_INPUT_[A-Z]+=1' | tr -s '\\n' ' '"))
        assert len(others) >= 2 and not any(alice_opens(node) for node in others), f"a hidden DualSense's touchpad and motion sensors: {others}"
        assert order(control, "raw") == "ok", "the raw node of a hidden pad of the PlayStation driver"
        assert order(control, "restore") == "ok"
        machine.succeed("udevadm settle")
        assert alice_can_open(DS, "event") and alice_can_open(DS, "hidraw"), "a restored DualSense"
        others = other_nodes(pads()[DS]["event"][0])
        assert len(others) >= 2 and all(alice_opens(node) for node in others), f"a restored DualSense's touchpad and motion sensors: {others}"
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

    with subtest("a hidden pad's node with an ACL entry beyond its mode isn't taken for hidden"):
        node = pads()[A]["event"][0]
        machine.succeed(f"setfacl -m u:bob:r {node}")
        told = order(control, "reopen")
        assert "couldn't carry the request out" in told, told
        machine.succeed(f"setfacl -b {node}")
        assert order(control, "reopen").startswith("ok")

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

    with subtest("a request longer than a request is refused, not read as its first five bytes"):
        told = machine.succeed(f"su -l alice -c 'hider-probe long {SOCKET}'")
        assert "status=7" in told, told

    with subtest("every refusal says how many re-binds it took, none, at the gate as well"):
        # Not understood, and from someone not in front of the machine.
        told = machine.succeed(f"su -l alice -c 'hider-probe long {SOCKET}'")
        assert "status=7 counted=1" in told, told
        told = machine.succeed(f"hider-probe long {SOCKET}")
        assert "status=1 counted=1" in told, told

    with subtest("run by hand with --listen, as the dev recipe does, it is the same service on a socket anyone may reach"):
        machine.succeed("install -d -o ctrlassist-hider -g ctrlassist-hider -m 0755 /run/ctrlassist-dev")
        machine.succeed(
            "systemd-run --unit hider-by-hand --uid ctrlassist-hider --gid ctrlassist-hider "
            "ctrlassist-hider --listen /run/ctrlassist-dev/hider.sock"
        )
        machine.wait_for_file("/run/ctrlassist-dev/hider.sock")
        assert machine.succeed("stat -c '%A %U' /run/ctrlassist-dev/hider.sock").strip() == "srw-rw-rw- ctrlassist-hider"
        told = machine.succeed("su -l alice -c 'ctrlassist-fixture version --socket /run/ctrlassist-dev/hider.sock'").strip()
        assert told == "version: ${hider.version}", told
        # Without the unit's sandbox, its user's permissions alone hide a pad.
        machine.succeed(
            f"systemd-run --unit by-hand --uid alice ctrlassist-fixture hide {pads()[A]['event'][0]} "
            "--socket /run/ctrlassist-dev/hider.sock --control /tmp/by-hand"
        )
        machine.wait_for_file("/tmp/by-hand/hide.done")
        told = machine.succeed("cat /tmp/by-hand/hide.done")
        assert told.startswith("ok") and "dislodged=true" in told, told
        assert not alice_can_open(A)
        assert order("/tmp/by-hand", "restore").startswith("ok")
        assert alice_can_open(A)
        machine.succeed("systemctl stop by-hand hider-by-hand")
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
        machine.succeed("systemd-run --unit xpad-gadget usb-gadget xpad xpad XPADFIXTURE1 0")
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
        # Taken out and put back in the same port while hidden: hidden by
        # its serial number as it comes back, and opened again.
        replug("xpad", 0, "X-Box")
        assert not alice_can_open(key, "event") and not alice_can_open(key, "js"), "an xpad pad that came back"
        assert order(control, "reopen").startswith("ok")
        assert order(control, "restore").startswith("ok")
        machine.succeed("udevadm settle")
        assert alice_can_open(key, "event") and alice_can_open(key, "js")
        machine.succeed("systemctl stop xpad")
        wait_restored()

    def by_name(name, as_user=None):
        """The nodes of the one pad of that name."""
        return [nodes for named, _, nodes in listing(as_user) if named == name][0]

    def all_are(nodes, access):
        return all(state == access for _, state in nodes.values())

    with subtest("a HID pad on USB is found again by its serial number after reconnecting, its raw node hidden throughout"):
        name = "Fixture Serial Pad"
        machine.succeed("systemd-run --unit gadget-serial usb-gadget hid serial HIDFIXTURE1 1 0x5ef0 'Serial Pad'")
        machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -c '{name}'", timeout=60)
        machine.succeed("udevadm settle")
        nodes = by_name(name, "alice")
        assert {"event", "js", "hidraw"} <= nodes.keys() and all_are(nodes, "open"), nodes
        interface = sysfs(nodes["event"][0], "device/..").split("/")[-1]
        control, told = hide("hide-serial", nodes["event"][0])
        assert told.startswith("ok"), told
        found = markers()
        print(f"a HID pad on USB with a serial number, on {interface}: " + " ".join(found))
        assert "./serial-1209-5ef0-HIDFIXTURE1" in found and f"./raw-{interface}-0003:00001209:00005EF0" in found, found
        assert f"./port-{interface}" not in found, "a pad with a serial number isn't hidden by its port"
        assert all_are(by_name(name, "alice"), "denied")
        replug("serial", 1, name)
        nodes = by_name(name, "alice")
        assert all_are(nodes, "denied"), f"a pad that came back on another HID device: {nodes}"
        assert order(control, "reopen").startswith("ok")
        assert order(control, "restore").startswith("ok")
        machine.succeed("udevadm settle")
        assert all_are(by_name(name, "alice"), "open")
        machine.succeed("systemctl stop hide-serial")
        wait_restored()

    with subtest("a HID pad on USB with no address or serial number is hidden by its port and found again by it, and a phys that climbs out of the marker directory leads nowhere"):
        name = "Fixture Port Pad"
        machine.succeed("systemd-run --unit gadget-port usb-gadget hid port - 2 0x5ef1 'Port Pad'")
        machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -c '{name}'", timeout=60)
        machine.succeed("udevadm settle")
        nodes = by_name(name, "alice")
        assert all_are(nodes, "open"), nodes
        interface = sysfs(nodes["event"][0], "device/..").split("/")[-1]
        phys = machine.succeed(f"cat {sysfs(nodes['event'][0], '.')}/phys").strip()
        control, told = hide("hide-port", nodes["event"][0])
        assert told.startswith("ok"), told
        found = markers()
        print(f"a HID pad on USB with neither, on {interface}, phys {phys}: " + " ".join(found))
        assert f"./port-{interface}" in found and f"./raw-{interface}-0003:00001209:00005EF1" in found, found
        assert all("/" not in marker[2:] for marker in found), "every marker is a file in the marker directory"
        assert all_are(by_name(name, "alice"), "denied")
        # A port marker as a directory named after the port's phys, as
        # the service once made, and a file outside the marker directory
        # that a phys with ../ in it reaches from there: a pad with that
        # phys isn't hidden. Under the rule that tested the phys, it was.
        port = phys.split("/")[0]
        machine.succeed(f"mkdir {MARKERS}/port-{port} && touch /run/canary")
        machine.succeed(f"systemd-run --unit pad-climbing hider-probe uhid 'Climbing Pad' '{port}/../../../canary'")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Climbing Pad'")
        machine.succeed("udevadm settle")
        climbing = by_name("Climbing Pad", "alice")
        climbing_phys = machine.succeed(f"cat {sysfs(climbing['event'][0], '.')}/phys").strip()
        print(f"a pad whose phys is {climbing_phys}: {climbing}")
        assert climbing["event"][1] == "open", "a pad whose phys climbs out of the marker directory"
        machine.succeed(f"systemctl stop pad-climbing; rmdir {MARKERS}/port-{port}; rm /run/canary")
        # Taken out and put back in the same port while hidden.
        replug("port", 2, name)
        nodes = by_name(name, "alice")
        assert all_are(nodes, "denied"), f"a pad that came back to the same port: {nodes}"
        assert order(control, "reopen").startswith("ok")
        assert order(control, "restore").startswith("ok")
        machine.succeed("udevadm settle")
        assert all_are(by_name(name, "alice"), "open")
        machine.succeed("systemctl stop hide-port")
        wait_restored()

    with subtest("a user who has switched away can't have a pad found again for her"):
        control, told = hide("switched", pads()[A]["event"][0])
        assert told.startswith("ok"), told
        machine.succeed("chvt 3")
        machine.wait_until_succeeds("[ -z \"$(loginctl show-seat seat0 -p ActiveSession --value)\" ]")
        machine.succeed("systemctl restart pad-a")
        machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -cF {A}")
        machine.succeed("udevadm settle")
        assert not alice_can_open(A)
        told = order(control, "reopen")
        assert "isn't the user in front" in told, told
        machine.succeed("chvt 1")
        machine.wait_until_succeeds("[ \"$(loginctl show-session $(loginctl show-seat seat0 -p ActiveSession --value) -p Name --value)\" = alice ]")
        assert order(control, "reopen").startswith("ok")
        assert order(control, "restore").startswith("ok")
        machine.succeed("systemctl stop switched")
        wait_restored()
        assert alice_can_open(A)

    def switch_away():
        machine.succeed("chvt 3")
        machine.wait_until_succeeds("[ -z \"$(loginctl show-seat seat0 -p ActiveSession --value)\" ]")

    def switch_back():
        machine.succeed("chvt 1")
        machine.wait_until_succeeds("[ \"$(loginctl show-session $(loginctl show-seat seat0 -p ActiveSession --value) -p Name --value)\" = alice ]")

    def unplug(gadget, gone):
        machine.succeed(f"echo > /sys/kernel/config/usb_gadget/{gadget}/UDC")
        machine.wait_until_fails(f"ctrlassist-fixture pads | grep -F {shlex.quote(gone)}")

    def plug(gadget, udc, name):
        machine.succeed(f"echo dummy_udc.{udc} > /sys/kernel/config/usb_gadget/{gadget}/UDC")
        machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -F {shlex.quote(name)}", timeout=60)
        machine.succeed("udevadm settle")

    with subtest("a pad on USB with no HID device is found again only as the same pad, and only for a user in front"):
        # An Xbox 360 pad with no serial number: recorded by its USB
        # interface, whose place is its port's, and hidden by its port.
        unplug("xpad", "X-Box")
        machine.succeed("systemd-run --unit gadget-xpad-port usb-gadget xpad xpadport - 3")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -F X-Box", timeout=60)
        machine.succeed("udevadm settle")
        key = [key for key in pads() if "X-Box" in key][0]
        node = pads()[key]["event"][0]
        interface = sysfs(node, "device").split("/")[-1]
        control, told = hide("switched-port", node)
        assert told.startswith("ok"), told
        assert markers() == [f"./port-{interface}"], markers()
        # Taken out and put back in while she is away: it could be anyone's
        # pad in that port, so it isn't found for her.
        switch_away()
        replug("xpadport", 3, "X-Box")
        told = order(control, "reopen")
        print(f"a reopen while switched away, a pad back in the port {interface}: {told.strip()}")
        assert "isn't the user in front" in told, told
        switch_back()
        # Another model in that port, the HID pad from before moved there:
        # hidden by the port's marker, and not taken for hers.
        unplug("xpadport", "X-Box")
        unplug("port", "Fixture Port Pad")
        plug("port", 3, "Fixture Port Pad")
        told = order(control, "reopen")
        print(f"a reopen in front, another model in the port: {told.strip()}")
        assert "isn't connected" in told, told
        # Her pad back in its port: found again, and shown again.
        unplug("port", "Fixture Port Pad")
        plug("xpadport", 3, "X-Box")
        assert order(control, "reopen").startswith("ok")
        assert order(control, "restore").startswith("ok")
        machine.succeed("udevadm settle")
        assert alice_can_open(key, "event")
        machine.succeed("systemctl stop switched-port")
        wait_restored()

    def raw_under(interface):
        """The raw node of the HID device under a USB interface."""
        return "/dev/" + machine.succeed(f"basename /sys/bus/usb/devices/{interface}/*/hidraw/hidraw*").strip()

    with subtest("a raw node is hidden by its port only with its model, and one left hidden is shown again once nothing names it"):
        # The HID pad with a serial number from before, still in its port.
        # The gadget driver has room for four HID functions, so the pads
        # made as gadgets are used again where they can be.
        name = "Fixture Serial Pad"
        nodes = by_name(name, "alice")
        assert all_are(nodes, "open"), nodes
        interface = sysfs(nodes["event"][0], "device/..").split("/")[-1]
        control, told = hide("hide-model", nodes["event"][0])
        assert told.startswith("ok"), told
        raw = [marker for marker in markers() if marker.startswith("./raw-")]
        assert raw == [f"./raw-{interface}-0003:00001209:00005EF0"], markers()
        # Taken out, and a keyboard plugged into its port: no node of the
        # keyboard is hidden, its raw node included, which a 60- rule opens.
        unplug("serial", name)
        machine.succeed("systemd-run --unit gadget-keyboard usb-gadget kbd keyboard 1 0x5efa")
        machine.wait_until_succeeds(f"ls /sys/bus/usb/devices/{interface}/*/hidraw", timeout=60)
        machine.succeed("udevadm settle")
        keyboard = raw_under(interface)
        print("a keyboard in a hidden pad's port: " + machine.succeed(f"stat -c '%n %a %U:%G' {keyboard}").strip())
        assert alice_opens(keyboard), "a keyboard's raw node in a hidden pad's port"
        machine.succeed("echo > /sys/kernel/config/usb_gadget/keyboard/UDC")
        machine.wait_until_fails(f"ls /sys/bus/usb/devices/{interface}")
        # Another pad of the model, with another serial number: its raw node
        # is hidden, as one of the model in that port, its event node not.
        machine.succeed("systemd-run --unit gadget-serial-other usb-gadget hid serialother HIDFIXTURE2 1 0x5ef0 'Serial Pad'")
        machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -F '{name}'", timeout=60)
        machine.succeed("udevadm settle")
        other = by_name(name, "alice")
        print(f"another pad of the model in the port: {other}")
        assert other["event"][1] == "open" and other["hidraw"][1] == "denied", other
        # The pad shown again while away: its markers go, nothing names the
        # other's raw node, and it is made again, open as the stock rules
        # and the 60- rule leave it.
        assert order(control, "restore").startswith("ok")
        machine.wait_until_succeeds(f"test -z \"$(ls -A {MARKERS})\"")
        machine.wait_until_succeeds(f"su -l alice -c 'ctrlassist-fixture pads' | grep -F '{name}' | grep -E 'hidraw=[^ ]*:open'")
        machine.succeed("udevadm settle")
        print(f"once nothing names it: {by_name(name, 'alice')}")
        assert all_are(by_name(name, "alice"), "open")
        machine.succeed("systemctl stop hide-model")
        unplug("serialother", name)
        wait_restored()

    with subtest("users with no seat share one cap however many user IDs they connect as, and leave room for the user at the seat"):
        control, told = hide("keeper-again", pads()[B]["event"][0])
        assert told.startswith("ok"), told
        pid = machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip()
        # As one user's subordinate IDs would: forty of them, with eight
        # connections each.
        machine.succeed(f"systemd-run --unit many-uids hider-probe uids {SOCKET} 200000 40 8 hold")
        machine.wait_until_succeeds("journalctl -u many-uids -o cat | grep -q answered=")
        told = machine.succeed("journalctl -u many-uids -o cat | grep answered=").strip()
        print(f"forty user IDs with no seat, eight connections each: {told}")
        assert told == "answered=32 closed=288", told
        # Another user with no seat finds no room; the user at the seat
        # does: her release told, a pad hidden and shown again.
        told = machine.succeed(f"hider-probe uids {SOCKET} 1001 1 1").strip()
        assert told == "answered=0 closed=1", told
        told = machine.succeed(f"su -l alice -c 'ctrlassist-fixture version --socket {SOCKET}'").strip()
        assert told == "version: ${hider.version}", told
        other, told = hide("served-again", pads()[A]["event"][0])
        assert told.startswith("ok"), told
        assert order(other, "restore").startswith("ok")
        machine.succeed("systemctl stop served-again")
        # She has room for what a daemon with sixteen pads holds, two for
        # each pad and two more, and a few over: forty in all, the one that
        # holds pad B among them, and not one more.
        machine.succeed(f"systemd-run --unit alice-many hider-probe uids {SOCKET} 1000 1 41 hold")
        machine.wait_until_succeeds("journalctl -u alice-many -o cat | grep -q answered=")
        told = machine.succeed("journalctl -u alice-many -o cat | grep answered=").strip()
        assert told == "answered=39 closed=2", told
        descriptors = int(machine.succeed(f"ls /proc/{pid}/fd | wc -l"))
        print(f"the service holds {descriptors} descriptors, with its limit at " + machine.succeed(f"grep 'Max open files' /proc/{pid}/limits").strip())
        assert descriptors < 200
        machine.succeed("systemctl stop alice-many")
        # Once they have gone there is room again.
        machine.succeed("systemctl stop many-uids")
        machine.wait_until_succeeds(f"hider-probe uids {SOCKET} 1001 1 1 | grep -x answered=1.closed=0")
        assert machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip() == pid, "the service didn't restart"
        assert order(control, "restore").startswith("ok")
        machine.succeed("systemctl stop keeper-again")
        wait_restored()

    def refused_at_once(unit, name, uniq):
        """Makes a pad, has alice hide it, and says what she was told."""
        fake(f"pad-{unit}", "fake-hid", "--name", name, "--uniq", uniq, "--pulse", "20")
        machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -c '{name}'")
        machine.succeed("udevadm settle")
        _, told = hide(unit, pads()[uniq]["event"][0])
        machine.wait_until_fails(f"systemctl is-active {unit}")
        return told

    with subtest("a hide the shipped rule doesn't carry out fails at once, and leaves the pad as it was"):
        # Its device's uevent file isn't the group's to write, as where
        # the rule isn't installed: refused before anything is marked.
        uniq = "02:00:00:00:02:01"
        fake("pad-unwritable", "fake-hid", "--name", "Unwritable Pad", "--uniq", uniq, "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Unwritable Pad'")
        machine.succeed("udevadm settle")
        uevent = sysfs(pads()[uniq]["event"][0], "device") + "/uevent"
        machine.succeed(f"chmod g-w {uevent}")
        _, told = hide("unwritable", pads()[uniq]["event"][0])
        assert "couldn't carry the request out" in told, told
        assert markers() == [] and alice_can_open(uniq)
        machine.succeed(f"chmod g+w {uevent}; systemctl stop pad-unwritable")
        # A rule installed that isn't the one shipped: it hides, but takes
        # no request to re-bind, so udev takes the request in and nothing
        # comes of it. In place for this pad alone: a copy of the rules
        # mounted over them, since a runtime rule can't override /etc's.
        uniq = "02:00:00:00:02:02"
        machine.succeed(
            "cp -rL /etc/udev/rules.d /run/test-rules && "
            "sed -i '/SYNTH_ARG_CTRLASSIST/,+1d' /run/test-rules/72-ctrlassist-hide.rules && "
            "! grep -q SYNTH_ARG /run/test-rules/72-ctrlassist-hide.rules && "
            "mount --bind /run/test-rules /etc/udev/rules.d && udevadm control --reload"
        )
        told = refused_at_once("unheeding", "Unheeding Pad", uniq)
        machine.succeed("umount /etc/udev/rules.d && rm -r /run/test-rules && udevadm control --reload")
        assert "couldn't carry the request out" in told, told
        wait_restored()
        assert alice_can_open(uniq)
        machine.succeed("systemctl stop pad-unheeding")
        # A later rule gives the made-again nodes seat access back.
        uniq = "02:00:00:00:02:03"
        told = refused_at_once("retagged", "Retagged Pad", uniq)
        assert "couldn't carry the request out" in told, told
        wait_restored()
        assert alice_can_open(uniq)
        machine.succeed("systemctl stop pad-retagged")
        print("the service on each: " + machine.succeed("journalctl -u ctrlassist-hider.service -o cat | grep -E 'uevent file|without re-binding|wasn.t hidden'").replace("\n", " | "))

    with subtest("a hide given up with a cancel leaves the pad as it was, and says how many times it re-bound the pad, which is how many times the pad went and came back"):
        uniq = "02:00:00:00:03:01"
        fake("pad-held", "fake-hid", "--name", "Held Pad", "--uniq", uniq, "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Held Pad'")
        machine.succeed("udevadm settle")
        sysname = pads()[uniq]["event"][0].split("/")[-1]
        hid = machine.succeed(f"basename $(readlink -f /sys/class/input/{sysname}/device/device)").strip()

        def input_number():
            """The number of the pad's input device: each re-bind that makes the pad go and come back makes it anew under the next."""
            return int(machine.succeed(f"ls /sys/bus/hid/devices/{hid}/input").strip().removeprefix("input"))

        # Cancelled while the re-bind is under way, and held there by a
        # rule udev runs for the node it makes: the service shows the pad
        # again by a second re-bind, held behind the first.
        machine.succeed("mkfifo -m 0600 /run/hold && mkfifo -m 0666 /run/go")
        before = input_number()
        node = pads()[uniq]["event"][0]
        machine.succeed(f"systemd-run --unit cancel-held --uid alice hider-probe cancel {SOCKET} {node} /run/go")
        machine.wait_until_succeeds("pgrep -f '/bin/cat /run/hold$'")
        assert markers() != [], "the pad is being hidden"
        machine.succeed("echo > /run/go")
        # The service took the cancel and is showing the pad again: its
        # markers are gone.
        machine.wait_until_succeeds(f"test -z \"$(ls -A {MARKERS})\"")
        machine.succeed("mv /run/hold /run/held && echo > /run/held && rm /run/held /run/go")
        machine.wait_until_succeeds("journalctl -u cancel-held -o cat | grep -q total=")
        told = machine.succeed("journalctl -u cancel-held -o cat | grep total=").strip()
        machine.succeed("udevadm settle")
        print(f"a hide cancelled as its re-bind was held: {told}; the pad's input went from input{before} to input{input_number()}")
        assert told == "hide=9 rebinds=2 cancel=0 rebinds=0 total=2", told
        assert input_number() - before == 2
        wait_restored()
        assert alice_can_open(uniq) and alice_can_open(uniq, "js")

        # Cancelled after the hide's reply, which shows it again; behind a
        # request sent during the hide, which is served after it and
        # doesn't give it up, so the cancel shows it again; and at once,
        # which the service may take before it asks for a re-bind, as it
        # waits for it, or once the hide is done. Each outcome is one of
        # these exactly, and the pad went and came back as many times as
        # it says.
        cancelled_after = "hide=0 rebinds=1 cancel=0 rebinds=1 total=2"
        for when, outcomes in [
            ("after", [cancelled_after]),
            ("ahead", ["hide=0 rebinds=1 version=0 cancel=0 rebinds=1 total=2"]),
            ("at-once", [
                "hide=9 rebinds=0 cancel=0 rebinds=0 total=0",
                "hide=9 rebinds=2 cancel=0 rebinds=0 total=2",
                cancelled_after,
            ]),
        ]:
            before = input_number()
            node = pads()[uniq]["event"][0]
            told = machine.succeed(f"su -l alice -c 'hider-probe cancel {SOCKET} {node} {when}'").strip()
            wait_restored()
            print(f"a hide cancelled {when}: {told}; the pad's input went from input{before} to input{input_number()}")
            assert told in outcomes, told
            assert input_number() - before == int(field(r"total=(\d+)", told)), told
            assert alice_can_open(uniq)

        # Cancelled before udev has taken the first re-bind: udev is held on
        # an earlier event of the pad's HID device, so the request waits in
        # its queue. The service waits for that request's answer before it
        # shows the pad again by a second, and a hide right after is done on
        # its own re-bind, and passes back a descriptor that still reads.
        machine.succeed("mkfifo -m 0600 /run/hold-early && mkfifo -m 0666 /run/go")
        machine.succeed(f"echo \"change $(cat /proc/sys/kernel/random/uuid) TESTHOLD=1\" > /sys/bus/hid/devices/{hid}/uevent")
        machine.wait_until_succeeds("pgrep -f '/bin/cat /run/hold-early$'")
        machine.succeed("systemd-run --unit seen-rebind hider-probe uevent SYNTH_ARG_CTRLASSIST=rebind")
        machine.wait_until_succeeds("journalctl -u seen-rebind -o cat | grep -q listening")
        before = input_number()
        node = pads()[uniq]["event"][0]
        machine.succeed(f"systemd-run --unit cancel-early --uid alice hider-probe cancel-then-hide {SOCKET} {node} {hid} /run/go")
        # The service has asked; udev hasn't taken it.
        machine.wait_until_fails("systemctl is-active seen-rebind")
        assert input_number() == before, "udev is held before the re-bind"
        machine.succeed("echo > /run/go")
        machine.wait_until_succeeds("journalctl -u cancel-early -o cat | grep -qx cancelled")
        assert input_number() == before, "udev is still held"
        machine.succeed("echo > /run/hold-early && rm /run/hold-early /run/go")
        machine.wait_until_succeeds("journalctl -u cancel-early -o cat | grep -q live=")
        told = machine.succeed("journalctl -u cancel-early -o cat | grep live=").strip()
        # Two for the hide given up, one for the hide after, and one for
        # showing that again as the client ended, which is after it said.
        machine.wait_until_succeeds(f"[ $(ls /sys/bus/hid/devices/{hid}/input | tr -dc 0-9) -ge {before + 4} ]")
        wait_restored()
        print(f"a hide cancelled before udev took its re-bind, then hidden again: {told}; the pad's input went from input{before} to input{input_number()}")
        assert told == "hide=9 rebinds=2 cancel=0 rebinds=0 again=0 rebinds=1 live=True", told
        assert input_number() - before == 4
        assert alice_can_open(uniq)
        machine.succeed("systemctl stop pad-held")

    with subtest("two re-binds of a pad asked back to back are run one after the other, and a hide asked behind another is done on its own"):
        uniq = "02:00:00:00:04:01"
        fake("pad-queued", "fake-hid", "--name", "Queued Pad", "--uniq", uniq, "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Queued Pad'")
        machine.succeed("udevadm settle")
        node = pads()[uniq]["event"][0]
        sysname = node.split("/")[-1]
        hid = machine.succeed(f"basename $(readlink -f /sys/class/input/{sysname}/device/device)").strip()
        inputs = f"ls /sys/bus/hid/devices/{hid}/input | tr -dc 0-9"
        before = int(machine.succeed(inputs))
        machine.succeed("systemd-run --unit queued-rebinds hider-probe rebinds 2")
        machine.wait_until_succeeds("journalctl -u queued-rebinds -o cat | grep -q listening")
        # udev held on an earlier event of the pad's HID device, so each
        # request after it waits in udev's queue.
        machine.succeed("mkfifo -m 0600 /run/hold-early")
        machine.succeed(f"echo \"change $(cat /proc/sys/kernel/random/uuid) TESTHOLD=1\" > /sys/bus/hid/devices/{hid}/uevent")
        machine.wait_until_succeeds("pgrep -f '/bin/cat /run/hold-early$'")
        # The first, asked as the service asks, as its user; then a hide.
        first = machine.succeed("cat /proc/sys/kernel/random/uuid").strip()
        machine.succeed(
            f"runuser -u ctrlassist-hider -- sh -c 'echo change {first} CTRLASSIST=rebind > /sys/bus/hid/devices/{hid}/uevent'"
        )
        machine.succeed(f"systemd-run --unit queued --uid alice ctrlassist-fixture hide {node} --socket {SOCKET} --control /tmp/queued")
        machine.wait_until_succeeds("[ $(journalctl -u queued-rebinds -o cat | grep -c '^K change') = 2 ]")
        assert int(machine.succeed(inputs)) == before, "udev is held before either re-bind"
        machine.succeed("echo > /run/hold-early && rm /run/hold-early")
        machine.wait_for_file("/tmp/queued/hide.done")
        told = machine.succeed("cat /tmp/queued/hide.done")
        machine.wait_until_fails("systemctl is-active queued-rebinds")
        seen = machine.succeed("journalctl -u queued-rebinds -o cat | sed '1,/^listening$/d'").split("\n")
        seen = [line for line in seen if line[:2] in ("K ", "U ")]
        second = seen[1].split()[-1]
        print(f"two re-binds asked back to back, the second a hide: {seen}; the hide: {told.strip()}")
        # Both queued before either ran; the first run, then answered;
        # only then the second run, then answered.
        assert seen == [
            f"K change {first}", f"K change {second}",
            "K unbind", "K bind", f"U change {first}",
            "K unbind", "K bind", f"U change {second}",
        ], seen
        # The hide is done on its own re-bind: its descriptor reads, and
        # the pad's input was made anew by both.
        assert told.startswith("ok"), told
        assert int(field(r"reads=(\d+)", order("/tmp/queued", "count"))) > 0
        machine.succeed("udevadm settle")
        assert int(machine.succeed(inputs)) - before == 2
        assert not alice_can_open(uniq)
        assert order("/tmp/queued", "restore").startswith("ok")
        machine.succeed("systemctl stop queued pad-queued")
        wait_restored()
  '';
}
