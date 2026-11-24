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
  # five bytes; a status, a token and a flag, six. Like the client library,
  # it asks the release on a connection before it passes a descriptor
  # there, the service taking none until it has admitted the connection,
  # except where it says it doesn't.
  #
  #   hider-probe eager <socket> <node>
  #       passes a descriptor on <node>, beside a version request, as soon
  #       as it has connected; says whether it was sent
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
  #   hider-probe lingering <socket> <count> [<more>]
  #       opens <count> connections and sends nothing, then one more that
  #       tries to pass a TCP socket whose close lingers, its data never
  #       read, before it is accepted; says whether it could, how many it
  #       holds and whether the last was closed; then opens <more>
  #       connections, sending nothing; holds
  #   hider-probe churn <socket> <count>
  #       <count> times without pause: connects, asks the release and
  #       closes; says how many were answered and how many refused
  #   hider-probe uhid <name> <phys> [<uniq> combo|merged|low|many|series|elite]
  #       makes a HID game pad through uhid with that phys and no address;
  #       holds. With a kind, with that address: with combo, a game pad
  #       and a keyboard in one descriptor, each its own report, product
  #       0bad; with merged, the keyboard's keys in the game pad's own
  #       report, which makes one input, product 0bac; with low, Enter and
  #       an arrow key in the game pad's own report, product 0baa; with
  #       many, eleven media keys there, all at 128 or above, product
  #       0bb9; with series, an Xbox Series pad on Bluetooth, 045e:0b13,
  #       its share button a key; with elite, an Xbox Elite Series 2 pad
  #       on Bluetooth, 045e:0b05, with a keyboard's collection
  #   hider-probe hang-up <socket> <node> <fifo>
  #       hides <node> and gives the hide up by closing the connection
  #       once something is written to <fifo>
  #   hider-probe stop-then-hide <socket> <node> <HID device> <fifo>
  #       hides <node>, gives the hide up by stopping sending once
  #       something is written to <fifo>, as the daemon does, and reads
  #       until the service closes its end; then hides the pad again on
  #       a new connection; says each status, and whether the descriptor
  #       the second hide passed back still reads
  #   hider-probe restore-then-stop <socket> <node> <fifo>
  #       hides <node>, says the status, and once something is written to
  #       <fifo> asks for it to be shown again and at once stops sending;
  #       reads until the service closes its end, and says each status
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

    # Real pads' report descriptors, as xpadneo's documents record them,
    # read from the pads over Bluetooth (docs/descriptors/xbxs.md and
    # xbe2_linux.md in xpadneo 0.10.5). An Xbox Series pad's carries its
    # share button in the game pad's own report, as the consumer page's
    # Record.
    XBOX_SERIES = bytes.fromhex(
        "05010905a10185010901a10009300931150027ff"
        "ff0000950275108102c00901a100093209351500"
        "27ffff0000950275108102c0050209c5150026ff"
        "039501750a810215002500750695018103050209"
        "c4150026ff039501750a81021500250075069501"
        "810305010939150125083500463b016614007504"
        "9501814275049501150025003500450065008103"
        "05091901290f150025017501950f810215002500"
        "750195018103050c0ab200150025019501750181"
        "0215002500750795018103050f09218503a10209"
        "9715002501750495019102150025007504950191"
        "030970150025647508950491020950660110550e"
        "150026ff0075089501910209a7150026ff007508"
        "9501910265005500097c150026ff007508950191"
        "02c0c0"
    )
    # An Xbox Elite Series 2's carries a boot keyboard's collection after
    # its game pad's, as report 5, and a zero byte at its end, which the
    # kernel passes over.
    ELITE_SERIES_2 = bytes.fromhex(
        "05010905a10185010901a10009300931150027ff"
        "ff0000950275108102c00901a100093209351500"
        "27ffff0000950275108102c0050209c5150026ff"
        "039501750a810215002500750695018103050209"
        "c4150026ff039501750a81021500250075069501"
        "810305010939150125083500463b016614007504"
        "9501814275049501150025003500450065008103"
        "05091901290f150025017501950f810215002500"
        "750195018103050c0a2402150025019501750181"
        "0215002500750795018103050c0901a1010a8100"
        "150026ff00950175048102150025009501750481"
        "030a8400150026ff009501750481021500250095"
        "01750481030a8500150026ff009501750881020a"
        "9900150026ff0095017504810215002500950175"
        "0481030a9e00150026ff009501750881020aa100"
        "150026ff009501750881020aa200150026ff0095"
        "01750881020aa300150026ff009501750881020a"
        "a400150026ff009501750881020ab900150026ff"
        "009501750881020aba00150026ff009501750881"
        "020abb00150026ff009501750881020abe001500"
        "26ff009501750881020ac000150026ff00950175"
        "0881020ac100150026ff009501750881020ac200"
        "150026ff009501750881020ac300150026ff0095"
        "01750881020ac400150026ff009501750881020a"
        "c500150026ff009501750881020ac600150026ff"
        "009501750881020ac700150026ff009501750881"
        "020ac800150026ff00950175088102c0050c0901"
        "8502a101050c0a23021500250195017501810215"
        "002500750795018103c0050f09218503a1020997"
        "1500250175049501910215002500750495019103"
        "0970150025647508950491020950660110550e15"
        "0026ff0075089501910209a7150026ff00750895"
        "01910265005500097c150026ff00750895019102"
        "c0050609208504150026ff007508950181020600"
        "ff0901a102850609011500256475089501b10209"
        "021500256475089501b1020903150026ff007508"
        "9501b102090426ff007508953cb20201c00600ff"
        "0902a102850709051500256475089501b1020906"
        "1500256475089501b10209071500256475089501"
        "b102c00600ff0903a10285080908150025647508"
        "9501b10209091500256475089501b102090a1500"
        "26ff0075089501b102c00600ff0904a101850909"
        "0b1500256475089501b102090c15002564750895"
        "01b102090d1500256475089501b102090e150026"
        "ff0075089501b102090f26ff007508953cb20201"
        "c00600ff0905a101850a0910150027ffffff7f75"
        "20950181020911150027ffffff7f752095018102"
        "0912150026ff007508950281020913150026ff00"
        "750895018102c00600ff0906a102850b09141500"
        "256475089501b102c0c005010906a10185050507"
        "19e029e715002501750195088102950175088103"
        "95067508150025650507190029658100c000"
    )


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


    def admitted(path):
        """A connection the service has answered, so may pass descriptors."""
        peer = connect(path)
        ask(peer, 6)
        return peer


    def pass_hide(peer, fd):
        """Sends a hide carrying fd, and reads no reply."""
        rights = [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", [fd]))]
        peer.sendmsg([struct.pack("<BI", 1, 0)], rights)


    def name_of(fd):
        node = os.readlink(f"/proc/self/fd/{fd}")
        base = os.path.basename(node)
        with open(f"/sys/class/input/{base}/device/name") as file:
            return node, file.read().strip()


    match sys.argv[1:]:
        case ["eager", path, node]:
            fd = os.open(node, os.O_RDONLY)
            peer = connect(path)
            try:
                # Beside a version request, which hides nothing.
                rights = [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", [fd]))]
                peer.sendmsg([struct.pack("<BI", 6, 0)], rights)
                print("sent=yes")
            except OSError as error:
                print(f"sent={error.strerror}")
        case ["opath", path, opcode, node]:
            fd = os.open(node, os.O_PATH)
            status, _, fds = ask(admitted(path), int(opcode), fd)
            print(f"status={status} fds={len(fds)}")
        case ["hide", path, node]:
            fd = os.open(node, os.O_RDONLY)
            status, token, fds = ask(admitted(path), 1, fd)
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
            print(f"status={data[0]}")
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
        case ["lingering", path, count, *more]:
            peers = [connect(path) for _ in range(int(count))]
            # A TCP connection whose other end never reads: once its window
            # is full, a close that lingers waits for data that can't go.
            server = socket.create_server(("127.0.0.1", 0))
            stuck = socket.create_connection(server.getsockname())
            reader, _ = server.accept()
            stuck.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 3600))
            stuck.setblocking(False)
            try:
                while True:
                    stuck.send(bytes(65536))
            except BlockingIOError:
                pass
            # Sent before the service accepts, so the service's end is the
            # last to hold it, where the kernel lets it be sent.
            last = connect(path)
            try:
                socket.send_fds(last, [struct.pack("<BI", 6, 0)], [stuck.fileno()])
                passed = "yes"
            except OSError as error:
                passed = error.strerror
                # The last to hold it here: closed with no linger.
                stuck.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
            print(f"passed={passed}", flush=True)
            stuck.close()
            try:
                closed = last.recv(6) == b""
            except ConnectionResetError:
                closed = True
            print(f"held={len(peers)} closed={closed}", flush=True)
            if more:
                after = [connect(path) for _ in range(int(more[0]))]
                print(f"more={len(after)}", flush=True)
            time.sleep(3600)
        case ["churn", path, count]:
            answered = refused = 0
            for _ in range(int(count)):
                try:
                    peer = connect(path)
                    peer.send(struct.pack("<BI", 6, 0))
                    answered += len(peer.recv(6)) == 6
                except OSError:
                    refused += 1
                    continue
                peer.close()
            print(f"answered={answered} refused={refused}", flush=True)
        case ["uhid", name, phys, *combo]:
            # The test crate's uhid pad's descriptor: a game pad.
            descriptor = bytes([
                0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0xA1, 0x00, 0x05, 0x09,
                0x19, 0x01, 0x29, 0x08, 0x15, 0x00, 0x25, 0x01, 0x95, 0x08,
                0x75, 0x01, 0x81, 0x02, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31,
                0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x02,
                0xC0, 0xC0,
            ])
            def keys(last):
                """A keyboard's keys: modifiers, a reserved byte and six keys, of usages up to <last>."""
                return bytes([
                    0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01,
                    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08,
                    0x81, 0x01, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, last,
                    0x05, 0x07, 0x19, 0x00, 0x29, last, 0x81, 0x00,
                ])
            # On USB, of the test vendor, unless a real pad's.
            uniq, bus, vendor, product = b"", 3, 0x1209, 0x0bae
            kind = combo[1] if combo else None
            if combo:
                uniq = combo[0].encode()
            if kind == "combo":
                # The same game pad as report 1, then a keyboard as report
                # 2.
                product = 0x0bad
                descriptor = (
                    descriptor[:6] + bytes([0x85, 0x01]) + descriptor[6:]
                    + bytes([0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x02]) + keys(0x65) + bytes([0xC0])
                )
            elif kind == "merged":
                # The keys inside the game pad's own collection: one input,
                # a joystick and a keyboard at once. Only those up to the
                # slash: udev takes a joystick with several of the keys past
                # it, Caps Lock among them, for a keyboard alone.
                product = 0x0bac
                descriptor = descriptor[:-1] + keys(0x38) + bytes([0xC0])
            elif kind == "low":
                # The keyboard page's Enter and up arrow, which the kernel
                # gives the keys KEY_ENTER (28) and KEY_UP (103), in two
                # bits and six of padding: a joystick with keys below 128,
                # and no keyboard.
                product = 0x0baa
                descriptor = descriptor[:-1] + bytes([
                    0x05, 0x07, 0x09, 0x28, 0x09, 0x52, 0x15, 0x00, 0x25, 0x01,
                    0x75, 0x01, 0x95, 0x02, 0x81, 0x02, 0x75, 0x06, 0x95, 0x01,
                    0x81, 0x01,
                ]) + bytes([0xC0])
            elif kind == "many":
                # Eleven keys of the consumer page, every one at 128 or
                # above: Play, Record, Fast Forward, Rewind, the next and
                # previous tracks, Stop, Eject, Shuffle, AC Home and AC
                # Back, in eleven bits and five of padding. A joystick with
                # ten keys or more, which libinput takes for a keyboard.
                product = 0x0bb9
                descriptor = descriptor[:-1] + bytes([
                    0x05, 0x0C, 0x09, 0xB0, 0x09, 0xB2, 0x09, 0xB3, 0x09, 0xB4,
                    0x09, 0xB5, 0x09, 0xB6, 0x09, 0xB7, 0x09, 0xB8, 0x09, 0xB9,
                    0x0A, 0x23, 0x02, 0x0A, 0x24, 0x02, 0x15, 0x00, 0x25, 0x01,
                    0x75, 0x01, 0x95, 0x0B, 0x81, 0x02, 0x75, 0x05, 0x95, 0x01,
                    0x81, 0x01,
                ]) + bytes([0xC0])
            elif kind == "series":
                # An Xbox Series pad on Bluetooth, for hid-microsoft.
                bus, vendor, product = 5, 0x045e, 0x0b13
                descriptor = XBOX_SERIES
            elif kind == "elite":
                # An Xbox Elite Series 2 pad on Bluetooth, for
                # hid-microsoft.
                bus, vendor, product = 5, 0x045e, 0x0b05
                descriptor = ELITE_SERIES_2
            elif combo:
                sys.exit(f"no such kind of uhid device: {kind}")
            uhid = os.open("/dev/uhid", os.O_RDWR)
            # UHID_CREATE2: name, phys, uniq, the descriptor's size, the
            # bus, vendor, product, version, country, the descriptor.
            os.write(uhid, struct.pack(
                "<I128s64s64sHHIIII4096s",
                11, name.encode(), phys.encode(), uniq, len(descriptor),
                bus, vendor, product, 1, 0, descriptor,
            ))
            print("created", flush=True)
            while True:
                os.read(uhid, 4380)
        case ["hang-up", path, node, go]:
            peer = admitted(path)
            fd = os.open(node, os.O_RDONLY)
            pass_hide(peer, fd)
            os.close(fd)
            with open(go) as waited:
                waited.read()
            peer.close()
            print("hung up", flush=True)
        case ["stop-then-hide", path, node, hid, go]:
            peer = admitted(path)
            fd = os.open(node, os.O_RDONLY)
            pass_hide(peer, fd)
            os.close(fd)
            with open(go) as waited:
                waited.read()
            peer.shutdown(socket.SHUT_WR)
            print("stopped sending", flush=True)
            # The hide's reply, then the end the service closes once it
            # has shown again what the connection hid.
            told = []
            while True:
                data, fds, _, _ = socket.recv_fds(peer, 6, 2)
                if not data:
                    break
                told.append(str(data[0]))
            peer.close()
            # Hidden again, on its node as it is now.
            [event] = glob.glob(f"/sys/bus/hid/devices/{hid}/input/input*/event*")
            fd = os.open("/dev/input/" + os.path.basename(event), os.O_RDONLY)
            peer = admitted(path)
            pass_hide(peer, fd)
            os.close(fd)
            data, fds, _, _ = socket.recv_fds(peer, 6, 2)
            # The pad reports every 20 ms: a live descriptor has something
            # to read by now, and one the pad's re-bind cut off fails.
            time.sleep(1)
            try:
                live = len(os.read(fds[0], 4096)) > 0
            except BlockingIOError:
                live = True
            except OSError:
                live = False
            print(f"hide={','.join(told)} again={data[0]} live={live}", flush=True)
        case ["restore-then-stop", path, node, go]:
            peer = admitted(path)
            fd = os.open(node, os.O_RDONLY)
            status, token, fds = ask(peer, 1, fd)
            os.close(fd)
            for fd in fds:
                os.close(fd)
            print(f"hidden={status}", flush=True)
            with open(go) as waited:
                waited.read()
            peer.send(struct.pack("<BI", 3, token))
            peer.shutdown(socket.SHUT_WR)
            told = []
            while True:
                data, fds, _, _ = socket.recv_fds(peer, 6, 2)
                if not data:
                    break
                told.append(str(data[0]))
            print(f"restore={','.join(told)}", flush=True)
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
            sys.exit("usage: hider-probe eager|opath|hide|long|silent|idle|uids|lingering|churn|uhid|hang-up|stop-then-hide|restore-then-stop|uevent|rebinds ...")
  '';
in
pkgs.testers.runNixOSTest {
  name = "ctrlassist-hider";

  nodes.machine = {
    boot.kernelModules = [
      "uinput"
      "uhid"
      "hid-playstation"
      # Loaded before its pads are made, so that the generic driver
      # doesn't take them first.
      "hid-microsoft"
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
    # The probe's uhid device that is a pad and a keyboard: an input of
    # each of its reports, as a real one with two collections can have.
    boot.kernelParams = [ "usbhid.quirks=0x1209:0x0bad:0x40" ];

    environment.systemPackages = [
      fixture
      gadget
      hider
      probe
      pkgs.acl
      # libinput's own tools, to see what a compositor would take.
      pkgs.libinput
    ];
  };

  testScript = ''
    import json
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

    def touchpad(event):
        """The touchpad's event node under the device a pad's event node hangs from."""
        found = [
            node for node in other_nodes(event)
            if node.split("/")[-1].startswith("event")
            and "ID_INPUT_TOUCHPAD=1" in machine.succeed(f"udevadm info -q property -n {node}").split()
        ]
        assert len(found) == 1, f"one touchpad under {event}: {found}"
        return found[0]

    def ignored(node):
        """Whether udev's record of a node tells libinput to leave it alone."""
        return "LIBINPUT_IGNORE_DEVICE=1" in machine.succeed(f"udevadm info -q property -n {node}").split()

    def libinput_lists(node):
        """Whether libinput, starting now, takes a node."""
        return re.search(rf"^Kernel:\s+{re.escape(node)}$", machine.succeed("libinput list-devices"), re.MULTILINE) is not None

    def compositor_holds(node):
        """A command that succeeds while the seat's compositor, which has logind open nodes for it, holds a descriptor on a node."""
        return f"ls -l /proc/$(systemctl show -p MainPID --value cage-tty1)/fd | grep -q ' {node}$'"

    def reader_mark():
        """Where libinput's reader, running since the machine started, has got to in its log."""
        return json.loads(machine.succeed("journalctl -u libinput-reader -n 1 -o json"))["__CURSOR"]

    def libinput_view(node, mark, taken):
        """What libinput makes of a node made since `mark`: whether udev's record of it says to leave it alone; whether libinput lists it, starting now; whether the reader, running all along, has it, by what it last did with the node, added it or let it go or left it alone; and whether the compositor holds it. Waits a minute at most for the last two to come to what `taken` says they should."""
        name = node.split("/")[-1]
        told = rf"\b{name}\b.*DEVICE_(ADDED|REMOVED)|\b{name}: device is ignored"
        log = f"journalctl -u libinput-reader -o cat --after-cursor={shlex.quote(mark)} | grep -E {shlex.quote(told)}"
        last = "DEVICE_ADDED" if taken else "device is ignored"
        holds = compositor_holds(node)
        try:
            machine.wait_until_succeeds(f"{log} | tail -1 | grep -qE {shlex.quote(last)} && " + (holds if taken else f"! ( {holds} )"), timeout=60)
        except Exception:
            pass
        lines = machine.succeed(f"{log} || true").splitlines()
        print(f"libinput's reader on {name} since the mark: {lines}")
        return dict(
            ignored=ignored(node),
            listed=libinput_lists(node),
            took=bool(lines) and "DEVICE_ADDED" in lines[-1],
            held=machine.execute(holds)[0] == 0,
        )

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
    # libinput, as a compositor runs it, from the start: told of each node
    # as it is added, which it takes or leaves alone then.
    machine.succeed("systemd-run --unit libinput-reader ${pkgs.coreutils}/bin/stdbuf -oL ${pkgs.libinput}/bin/libinput debug-events --verbose")
    machine.wait_until_succeeds("journalctl -u libinput-reader -o cat | grep -q DEVICE_ADDED")
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
        # The listener systemd passed, its first descriptor, is blocking,
        # NonBlocking= being off: an accept waits for a client.
        listener = machine.succeed(f"cat /proc/{pid}/fdinfo/3")
        assert int(field(r"flags:\s+([0-7]+)", listener), 8) & 0o4000 == 0, listener
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
        mark = reader_mark()
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
        # Its touchpad is a pointer to libinput, and so to the compositor,
        # which holds it.
        node = touchpad(dualsense["event"][0])
        view = libinput_view(node, mark, taken=True)
        print(f"a DualSense's touchpad, shown: {node} {view}")
        assert view == dict(ignored=False, listed=True, took=True, held=True), view
        # Hidden, its raw node is closed to the seat user, though another
        # rule opens it, and still to be had by whoever hid it; a hidden
        # pad of another driver's isn't.
        mark = reader_mark()
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
        # Its touchpad, made anew by the re-bind, is left alone by libinput,
        # though logind would open it for the compositor, the node's group
        # notwithstanding.
        node = touchpad(pads()[DS]["event"][0])
        view = libinput_view(node, mark, taken=False)
        print(f"a DualSense's touchpad, hidden: {node} {view}")
        assert view == dict(ignored=True, listed=False, took=False, held=False), view
        assert order(control, "raw") == "ok", "the raw node of a hidden pad of the PlayStation driver"
        # Unplugged and plugged back in while hidden, its HID device made
        # anew: every node of it is hidden, its touchpad's and motion
        # sensors' as well as its joystick's, the first two by the change
        # event the joystick's event has udev send them.
        mark = reader_mark()
        machine.succeed("systemctl restart pad-ds")
        machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -F {DS} | grep -q hidraw=")
        machine.succeed("udevadm settle")
        assert not alice_can_open(DS, "event") and not alice_can_open(DS, "hidraw"), "a DualSense hidden again as it reconnects"
        others = other_nodes(pads()[DS]["event"][0])
        for node in others:
            print("reconnected DualSense's other node: " + machine.succeed(f"stat -c '%n %a %U:%G' {node}").strip())
        assert len(others) >= 2 and not any(alice_opens(node) for node in others), f"a reconnected DualSense's touchpad and motion sensors: {others}"
        # Its touchpad, which libinput and the compositor took as it was
        # added, before the change event hid it, is let go, udev telling
        # them it went and came back.
        node = touchpad(pads()[DS]["event"][0])
        view = libinput_view(node, mark, taken=False)
        print(f"a DualSense's touchpad, reconnected while hidden: {node} {view}")
        assert view == dict(ignored=True, listed=False, took=False, held=False), view
        assert order(control, "reopen").startswith("ok")
        mark = reader_mark()
        assert order(control, "restore") == "ok"
        machine.succeed("udevadm settle")
        assert alice_can_open(DS, "event") and alice_can_open(DS, "hidraw"), "a restored DualSense"
        others = other_nodes(pads()[DS]["event"][0])
        assert len(others) >= 2 and all(alice_opens(node) for node in others), f"a restored DualSense's touchpad and motion sensors: {others}"
        # Shown again, its touchpad is made anew with nothing that says to
        # leave it alone, and the compositor takes it up.
        node = touchpad(pads()[DS]["event"][0])
        view = libinput_view(node, mark, taken=True)
        print(f"a DualSense's touchpad, shown again: {node} {view}")
        assert view == dict(ignored=False, listed=True, took=True, held=True), view
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
        assert told.startswith("ok"), told
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
        # Its own listener refuses descriptors, as the socket unit's does.
        machine.succeed("journalctl -u hider-by-hand -o cat | grep -q 'connections pass no descriptor until they are admitted'")
        # Without the unit's sandbox, its user's permissions alone hide a pad.
        machine.succeed(
            f"systemd-run --unit by-hand --uid alice ctrlassist-fixture hide {pads()[A]['event'][0]} "
            "--socket /run/ctrlassist-dev/hider.sock --control /tmp/by-hand"
        )
        machine.wait_for_file("/tmp/by-hand/hide.done")
        told = machine.succeed("cat /tmp/by-hand/hide.done")
        assert told.startswith("ok"), told
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

    def watching(unit):
        """Starts the daemon's monitor, as the fixture's watch, writing what it hears to /tmp/<unit>."""
        machine.succeed(f"systemd-run --unit {unit} -p StandardOutput=file:/tmp/{unit} ctrlassist-fixture watch")
        machine.wait_until_succeeds(f"grep -qx watching /tmp/{unit}")

    def heard(unit, pattern):
        """Waits for the monitor of `watching` to have said a line `pattern`, an extended regular expression, matches whole."""
        machine.wait_until_succeeds(f"grep -qxE {shlex.quote(pattern)} /tmp/{unit}")

    with subtest("an Xbox 360 pad on USB under xpad is hidden and shown again by re-binding its USB interface"):
        watching("watch-xpad")
        machine.succeed("systemd-run --unit xpad-gadget usb-gadget xpad xpad XPADFIXTURE1 0")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c X-Box", timeout=60)
        machine.succeed("udevadm settle")
        key = [key for key in pads() if "X-Box" in key][0]
        node = pads()[key]["event"][0]
        sysname = node.split("/")[-1]
        # The daemon's monitor anchors a pad with no HID device to its USB
        # interface, the device the service re-binds.
        interface = sysfs(node, "device")
        heard("watch-xpad", f"arrived {node} \\S+ anchor={interface}")
        print("xpad pad: " + machine.succeed(
            f"udevadm info -q property -n {node} | grep -E '^(ID_SERIAL_SHORT|ID_BUS|CURRENT_TAGS)='"
        ).replace("\n", " ") + " driver=" + machine.succeed(f"basename $(readlink -f /sys/class/input/{sysname}/device/device/driver)").strip())
        assert alice_can_open(key, "event")
        listening("early-xpad", node)
        control, told = hide("xpad", node)
        assert told.startswith("ok"), told
        assert cut_off("early-xpad"), "the early reader kept reading"
        assert not alice_can_open(key, "event") and not alice_can_open(key, "js")
        assert "./serial-045e-028e-XPADFIXTURE1" in markers(), markers()
        # Taken out and put back in the same port while hidden: hidden by
        # its serial number as it comes back, and opened again.
        replug("xpad", 0, "X-Box")
        assert not alice_can_open(key, "event") and not alice_can_open(key, "js"), "an xpad pad that came back"
        # Taken out, its anchor is heard to go, the pad unplugged; back in
        # the same port, it is anchored there again.
        heard("watch-xpad", f"unplugged {interface}")
        machine.succeed("systemctl stop watch-xpad")
        print("the monitor through the xpad pad's hide and replug:\n" + machine.succeed("cat /tmp/watch-xpad"))
        assert machine.succeed("tac /tmp/watch-xpad | grep -m1 '^arrived '").strip().endswith(f" anchor={interface}")
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
        watching("watch-serial")
        machine.succeed("systemd-run --unit gadget-serial usb-gadget hid serial HIDFIXTURE1 1 0x5ef0 'Serial Pad'")
        machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -c '{name}'", timeout=60)
        machine.succeed("udevadm settle")
        nodes = by_name(name, "alice")
        assert {"event", "js", "hidraw"} <= nodes.keys() and all_are(nodes, "open"), nodes
        interface = sysfs(nodes["event"][0], "device/..").split("/")[-1]
        # A HID pad on USB is anchored to its HID device, not the USB
        # interface above it.
        hid_device = sysfs(nodes["event"][0], "device")
        heard("watch-serial", f"arrived {nodes['event'][0]} \\S+ anchor={hid_device}")
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
        # Taken out, its HID device is heard to go, the pad unplugged.
        heard("watch-serial", f"unplugged {hid_device}")
        machine.succeed("systemctl stop watch-serial")
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

    with subtest("a connection takes no descriptor until the service has answered on it, and a client that asks first hides a pad as the service starts"):
        # The service says what it made of its listener as it started.
        machine.succeed("journalctl -u ctrlassist-hider.service -o cat | grep -q 'connections pass no descriptor until they are admitted'")
        # Queued as the service starts, a connection has the listener's
        # setting: a descriptor passed at once, as by a client that asks
        # nothing first, is refused by the kernel before it is sent.
        machine.succeed("systemctl stop ctrlassist-hider.service")
        told = machine.succeed(f"su -l alice -c 'hider-probe eager {SOCKET} {pads()[A]['event'][0]}'").strip()
        assert told == "sent=Operation not permitted", told
        # The client library asks the release first, and passes the pad
        # once it is answered: the first client the service ever has
        # hides a pad.
        machine.succeed("systemctl stop ctrlassist-hider.service")
        control, told = hide("cold", pads()[A]["event"][0])
        assert told.startswith("ok"), told
        assert order(control, "restore").startswith("ok")
        machine.succeed("systemctl stop cold")
        wait_restored()

    with subtest("a user with no seat can pass no descriptor before being admitted, so nothing they send holds the closer, and the user at the seat is served at once"):
        # What the closer alone stood against before: eight connections
        # held, a ninth that passes a TCP socket whose close lingers before
        # it is accepted, then 65 more, which would fill the closer's queue
        # behind that close and have the main thread keep the last and
        # accept nothing more. Sent as the service starts, as before anyone
        # has used it since boot: the kernel refuses the socket at the send.
        machine.succeed("systemctl stop ctrlassist-hider.service")
        machine.succeed(f"systemd-run --unit flood --uid bob hider-probe lingering {SOCKET} 8 65")
        machine.wait_until_succeeds("journalctl -u flood -o cat | grep -q more=")
        told = " ".join(machine.succeed("journalctl -u flood -o cat | grep -E 'passed=|held=|more='").split())
        assert told == "passed=Operation not permitted held=8 closed=True more=65", told
        pid = machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip()
        # The user at the seat is answered at once, and hides a pad.
        told = machine.succeed(f"su -l alice -c 'ctrlassist-fixture version --socket {SOCKET}'").strip()
        assert told == "version: ${hider.version}", told
        control, told = hide("past-flood", pads()[A]["event"][0])
        assert told.startswith("ok"), told
        assert order(control, "restore").startswith("ok")
        # The closer never lingered in a close, and the main thread waits
        # on the listener as well as the closings: a ppoll of two.
        closer = f"for task in /proc/{pid}/task/*; do if grep -qx closer $task/comm; then cat $task/stack; fi; done"
        assert machine.execute(f"{closer} | grep -q tcp_close")[0] != 0, machine.succeed(closer)
        machine.wait_until_succeeds(f"test \"$(cut -d' ' -f1,3 /proc/{pid}/syscall)\" = '271 0x2'")
        assert machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip() == pid, "the service didn't restart"
        machine.succeed("systemctl stop past-flood flood")
        wait_restored()

    with subtest("a listener that takes descriptors, as systemd before 258 leaves it, is made to refuse them as the service starts"):
        # As where systemd doesn't know the socket unit's setting.
        machine.succeed("systemctl stop ctrlassist-hider.service ctrlassist-hider.socket")
        machine.succeed("mkdir -p /run/systemd/system/ctrlassist-hider.socket.d")
        machine.succeed("printf '[Socket]\\nAcceptFileDescriptors=yes\\n' > /run/systemd/system/ctrlassist-hider.socket.d/takes.conf")
        machine.succeed("systemctl daemon-reload")
        machine.succeed("systemctl start ctrlassist-hider.socket")
        # Queued before the service has started, a connection still has
        # the listener's setting from then: its descriptor is sent.
        told = machine.succeed(f"su -l alice -c 'hider-probe eager {SOCKET} {pads()[A]['event'][0]}'").strip()
        assert told == "sent=yes", told
        machine.wait_until_succeeds("journalctl -u ctrlassist-hider.service -o cat | grep -q 'the listener took descriptors'")
        control, told = hide("taking", pads()[A]["event"][0])
        assert told.startswith("ok"), told
        assert order(control, "restore").startswith("ok")
        machine.succeed("systemctl stop taking")
        wait_restored()
        machine.succeed("systemctl stop ctrlassist-hider.service ctrlassist-hider.socket")
        machine.succeed("rm /run/systemd/system/ctrlassist-hider.socket.d/takes.conf")
        machine.succeed("systemctl daemon-reload")
        machine.succeed("systemctl start ctrlassist-hider.socket")

    with subtest("connecting and closing without pause while nothing is hidden neither starts the service again nor puts the socket out of service"):
        # Started once, the service stays, so the starts systemd limits
        # are nobody's to make: two hundred connections from a user with
        # no seat, each closed at once, ten times systemd's default limit
        # on a socket's starts, then twenty of the user at the seat's
        # own, all served by the one process.
        told = machine.succeed(f"su -l alice -c 'ctrlassist-fixture version --socket {SOCKET}'").strip()
        assert told == "version: ${hider.version}", told
        pid = machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip()
        assert pid != "0"
        started = "journalctl -u ctrlassist-hider.service -o cat | grep -c '^Started CtrlAssist hiding service'"
        before = int(machine.succeed(started))
        told = machine.succeed(f"runuser -u bob -- hider-probe churn {SOCKET} 200").strip()
        assert told == "answered=200 refused=0", told
        told = machine.succeed(f"su -l alice -c 'hider-probe churn {SOCKET} 20'").strip()
        assert told == "answered=20 refused=0", told
        after = int(machine.succeed(started))
        print(f"two hundred and twenty connections, each closed at once: the service started {after - before} times; the socket: " + machine.succeed("systemctl show -p ActiveState -p Result ctrlassist-hider.socket").replace("\n", " "))
        assert after == before, after - before
        assert machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip() == pid, "the service started again"
        machine.succeed("systemctl is-active ctrlassist-hider.socket")
        control, told = hide("after-churn", pads()[A]["event"][0])
        assert told.startswith("ok"), told
        assert order(control, "restore").startswith("ok")
        machine.succeed("systemctl stop after-churn")
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

    with subtest("a hide given up by hanging up in the middle leaves the pad shown again, and a hide right after is done on its own re-bind"):
        uniq = "02:00:00:00:03:01"
        fake("pad-held", "fake-hid", "--name", "Held Pad", "--uniq", uniq, "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Held Pad'")
        machine.succeed("udevadm settle")
        sysname = pads()[uniq]["event"][0].split("/")[-1]
        hid = machine.succeed(f"basename $(readlink -f /sys/class/input/{sysname}/device/device)").strip()

        def input_number():
            """The number of the pad's input device: each re-bind that makes the pad go and come back makes it anew under the next."""
            return int(machine.succeed(f"ls /sys/bus/hid/devices/{hid}/input").strip().removeprefix("input"))

        # Hung up while the re-bind is under way, and held there by a rule
        # udev runs for the node it makes: the service stops waiting and
        # shows the pad again by a second re-bind, held behind the first.
        machine.succeed("mkfifo -m 0600 /run/hold && mkfifo -m 0666 /run/go")
        before = input_number()
        node = pads()[uniq]["event"][0]
        machine.succeed(f"systemd-run --unit hang-up-held --uid alice hider-probe hang-up {SOCKET} {node} /run/go")
        machine.wait_until_succeeds("pgrep -f '/bin/cat /run/hold$'")
        assert markers() != [], "the pad is being hidden"
        machine.succeed("echo > /run/go")
        machine.wait_until_succeeds("journalctl -u hang-up-held -o cat | grep -qx 'hung up'")
        # The service saw the hang-up and is showing the pad again: its
        # markers are gone.
        machine.wait_until_succeeds(f"test -z \"$(ls -A {MARKERS})\"")
        machine.succeed("mv /run/hold /run/held && echo > /run/held && rm /run/held /run/go")
        wait_restored()
        print(f"a hide hung up on as its re-bind was held: the pad's input went from input{before} to input{input_number()}")
        assert input_number() - before == 2
        assert alice_can_open(uniq) and alice_can_open(uniq, "js")

        # Given up before udev has taken the first re-bind, by stopping
        # sending, as the daemon does: udev is held on an earlier event of
        # the pad's HID device, so the request waits in its queue. The
        # service shows the pad again by a second re-bind, which udev runs
        # after the first, and closes its end once it has; a hide right
        # after is done on its own re-bind, and passes back a descriptor
        # that still reads.
        machine.succeed("mkfifo -m 0600 /run/hold-early && mkfifo -m 0666 /run/go")
        machine.succeed(f"echo \"change $(cat /proc/sys/kernel/random/uuid) TESTHOLD=1\" > /sys/bus/hid/devices/{hid}/uevent")
        machine.wait_until_succeeds("pgrep -f '/bin/cat /run/hold-early$'")
        machine.succeed("systemd-run --unit seen-rebind hider-probe uevent SYNTH_ARG_CTRLASSIST=rebind")
        machine.wait_until_succeeds("journalctl -u seen-rebind -o cat | grep -q listening")
        before = input_number()
        node = pads()[uniq]["event"][0]
        machine.succeed(f"systemd-run --unit stop-early --uid alice hider-probe stop-then-hide {SOCKET} {node} {hid} /run/go")
        # The service has asked; udev hasn't taken it.
        machine.wait_until_fails("systemctl is-active seen-rebind")
        assert input_number() == before, "udev is held before the re-bind"
        machine.succeed("echo > /run/go")
        machine.wait_until_succeeds("journalctl -u stop-early -o cat | grep -qx 'stopped sending'")
        # The service saw the hang-up and is showing the pad again, its
        # markers gone, while udev is still held.
        machine.wait_until_succeeds(f"test -z \"$(ls -A {MARKERS})\"")
        assert input_number() == before, "udev is still held"
        machine.succeed("echo > /run/hold-early && rm /run/hold-early /run/go")
        machine.wait_until_succeeds("journalctl -u stop-early -o cat | grep -q live=")
        told = machine.succeed("journalctl -u stop-early -o cat | grep live=").strip()
        # One for the hide given up and one for showing it again, one for
        # the hide after, and one for showing that again as the client
        # ended, which is after it said.
        machine.wait_until_succeeds(f"[ $(ls /sys/bus/hid/devices/{hid}/input | tr -dc 0-9) -ge {before + 4} ]")
        wait_restored()
        print(f"a hide given up before udev took its re-bind, then hidden again: {told}; the pad's input went from input{before} to input{input_number()}")
        # The hide given up is refused as not carried out; the one after
        # is done.
        assert told == "hide=6 again=0 live=True", told
        assert input_number() - before == 4
        assert alice_can_open(uniq)
        machine.succeed("systemctl stop pad-held")

    with subtest("a showing again given up by hanging up asks one re-bind, which shows the pad, and the sweep asks none of its own"):
        uniq = "02:00:00:00:03:02"
        fake("pad-restored", "fake-hid", "--name", "Restored Pad", "--uniq", uniq, "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Restored Pad'")
        machine.succeed("udevadm settle")
        sysname = pads()[uniq]["event"][0].split("/")[-1]
        hid = machine.succeed(f"basename $(readlink -f /sys/class/input/{sysname}/device/device)").strip()
        inputs = f"ls /sys/bus/hid/devices/{hid}/input | tr -dc 0-9"
        machine.succeed("mkfifo -m 0666 /run/go")
        machine.succeed(f"systemd-run --unit restore-given-up --uid alice hider-probe restore-then-stop {SOCKET} {pads()[uniq]['event'][0]} /run/go")
        machine.wait_until_succeeds("journalctl -u restore-given-up -o cat | grep -qx hidden=0")
        machine.succeed("udevadm settle")
        before = int(machine.succeed(inputs))
        # udev held on an earlier event of the pad's HID device, so the
        # showing's re-bind waits in its queue; then the restore, and the
        # hang-up behind it.
        machine.succeed("mkfifo -m 0600 /run/hold-early")
        machine.succeed(f"echo \"change $(cat /proc/sys/kernel/random/uuid) TESTHOLD=1\" > /sys/bus/hid/devices/{hid}/uevent")
        machine.wait_until_succeeds("pgrep -f '/bin/cat /run/hold-early$'")
        machine.succeed("echo > /run/go")
        # The service stops waiting, answers and closes its end with its
        # re-bind still queued, its markers gone.
        machine.wait_until_succeeds("journalctl -u restore-given-up -o cat | grep -q restore=")
        told = machine.succeed("journalctl -u restore-given-up -o cat | grep restore=").strip()
        assert told == "restore=0", told
        assert int(machine.succeed(inputs)) == before, "udev is still held"
        assert markers() == [], markers()
        machine.succeed("echo > /run/hold-early && rm /run/hold-early /run/go")
        wait_restored()
        after = int(machine.succeed(inputs))
        print(f"a showing again given up by hanging up: the pad's input went from input{before} to input{after}")
        # The one re-bind asked showed it; the sweep after asked no other.
        assert after - before == 1, (before, after)
        assert alice_can_open(uniq) and alice_can_open(uniq, "js")
        machine.succeed("systemctl stop pad-restored")

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

    with subtest("a device with a keyboard beside its pad is left as it is by the sweep, as a hide refuses it"):
        uniq = "02:00:00:00:05:01"
        fake("pad-shared", "fake-hid", "--name", "Shared Address Pad", "--uniq", uniq, "--pulse", "20")
        machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c 'Shared Address Pad'")
        machine.succeed("udevadm settle")
        control, told = hide("shared", pads()[uniq]["event"][0])
        assert told.startswith("ok"), told
        # A joystick and a keyboard, each an input of one HID device, with
        # the hidden pad's address: its joystick node is hidden by the
        # pad's marker, its keyboard's is not.
        machine.succeed("modprobe usbhid && grep -q 0bad /sys/module/usbhid/parameters/quirks")
        machine.succeed(f"systemd-run --unit combo hider-probe uhid 'Combo Pad' combo {uniq} combo")
        machine.wait_until_succeeds("[ $(ls -d /sys/bus/hid/devices/0003:1209:0BAD.*/input/input* | wc -l) = 2 ]")
        machine.succeed("udevadm settle")

        def combo():
            """Each event node of the device: its input, whether it is a keyboard, and its group."""
            return machine.succeed(
                "for event in /sys/bus/hid/devices/0003:1209:0BAD.*/input/input*/event*; do "
                "node=/dev/input/$(basename $event); "
                "echo $(basename $(dirname $event)) $(udevadm info -q property -n $node | grep -c '^ID_INPUT_KEYBOARD=1') $(stat -c %G $node); "
                "done"
            ).strip().split("\n")

        was = combo()
        print(f"a pad and a keyboard on one device, with a hidden pad's address: {was}")
        groups = dict(line.split()[1:] for line in was)
        assert len(was) == 2 and groups["0"] == "ctrlassist-hider" and groups["1"] != "ctrlassist-hider", was
        # One input that is a joystick and a keyboard at once, with the
        # hidden pad's address: no marker hides its nodes, as the service
        # would refuse to hide it and nothing would show it again.
        machine.succeed(f"systemd-run --unit merged hider-probe uhid 'Merged Pad' merged {uniq} merged")
        machine.wait_until_succeeds("ls /sys/bus/hid/devices/0003:1209:0BAC.*/input/input*/event*")
        machine.succeed("udevadm settle")
        merged = machine.succeed(
            "for path in $(ls -d /sys/bus/hid/devices/0003:1209:0BAC.*/input/input*/event* /sys/bus/hid/devices/0003:1209:0BAC.*/input/input*/js* 2>/dev/null); do "
            "node=/dev/input/$(basename $path); "
            "echo $(basename $path) $(udevadm info -q property -n $node | grep -cE '^ID_INPUT_(JOYSTICK|KEYBOARD)=1') $(stat -c %G $node); "
            "done"
        ).strip().split("\n")
        print(f"a joystick that udev also takes for a keyboard, with a hidden pad's address: {merged}")
        assert any(line.startswith("event") for line in merged), merged
        assert all(line.split()[1:] == ["2", "input"] for line in merged), merged
        # A joystick with keys below 128, Enter and an arrow, and no
        # keyboard, with the hidden pad's address: the service won't take
        # it for a pad, so no marker hides it on its own account.
        machine.succeed(f"systemd-run --unit low hider-probe uhid 'Low Key Pad' low {uniq} low")
        machine.wait_until_succeeds("ls /sys/bus/hid/devices/0003:1209:0BAA.*/input/input*/event*")
        machine.succeed("udevadm settle")
        low = machine.succeed(
            "for path in $(ls -d /sys/bus/hid/devices/0003:1209:0BAA.*/input/input*/event* /sys/bus/hid/devices/0003:1209:0BAA.*/input/input*/js* 2>/dev/null); do "
            "node=/dev/input/$(basename $path); "
            "echo $(basename $path) $(udevadm info -q property -n $node | grep -E '^ID_INPUT_(JOYSTICK|KEY|KEYBOARD)=1' | sort | tr '\\n' ' ') $(stat -c %G $node); "
            "done"
        ).strip().split("\n")
        keys = machine.succeed("cat /sys/bus/hid/devices/0003:1209:0BAA.*/input/input*/capabilities/key").strip()
        print(f"a joystick with Enter and an arrow key, with a hidden pad's address: {low}, keys {keys}")
        assert len(low) == 2 and all(line.split()[1:] == ["ID_INPUT_JOYSTICK=1", "ID_INPUT_KEY=1", "input"] for line in low), low
        assert keys == "ff000000000000 0 0 8000000000 10000000", keys
        event = "/dev/input/" + next(line.split()[0] for line in low if line.startswith("event"))
        assert "refused" in machine.succeed(f"ctrlassist-hider check {event}"), "the service takes a joystick with a key below 128 for no pad"
        # The pad shown again: nothing names the device's joystick node any
        # more, and the sweep leaves it hidden rather than re-bind the
        # keyboard away.
        # The sweep is done before the restore's reply.
        assert order(control, "restore").startswith("ok")
        machine.succeed("udevadm settle")
        assert combo() == was, combo()
        machine.succeed("journalctl -u ctrlassist-hider.service -o cat | grep -q 'left hidden a pad no marker names'")
        machine.succeed("systemctl stop shared combo merged low pad-shared")
        wait_restored()

    with subtest("a pad whose keys are all at 128 or above, as an Xbox Series pad's share button is, is hidden and shown again as a pad, and one with a keyboard's collection is never hidden"):
        uniq = "02:00:00:00:06:01"
        SERIES, ELITE, MANY = "0005:045E:0B13", "0005:045E:0B05", "0003:1209:0BB9"

        def hid_devices(ids):
            return set(machine.succeed(f"cd /sys/bus/hid/devices && ls -d {ids}.* 2>/dev/null || true").split())

        def made(unit, name, kind, ids):
            """Makes a uhid device of a kind with the address, and returns its HID device's name once udev is done with it."""
            before = hid_devices(ids)
            machine.succeed(f"systemd-run --unit {unit} hider-probe uhid {shlex.quote(name)} {kind} {uniq} {kind}")
            machine.wait_until_succeeds(f"[ $(cd /sys/bus/hid/devices && ls -d {ids}.* | wc -l) -gt {len(before)} ]")
            [hid] = hid_devices(ids) - before
            machine.wait_until_succeeds(f"ls /sys/bus/hid/devices/{hid}/input/input*/event*")
            machine.succeed("udevadm settle")
            return hid

        def nodes_of(hid):
            """Each event, js and raw node of a HID device: its group, and the classes udev gives it."""
            found = {}
            paths = machine.succeed(f"cd /sys/bus/hid/devices/{hid} && ls -d input/input*/event* input/input*/js* hidraw/hidraw* 2>/dev/null || true").split()
            for path in paths:
                name = path.split("/")[-1]
                node = ("/dev/" if name.startswith("hidraw") else "/dev/input/") + name
                classes = machine.succeed(f"udevadm info -q property -n {node} | grep -E '^ID_INPUT_[A-Z]+=1' | sort || true").split()
                found[node] = (machine.succeed(f"stat -c %G {node}").strip(), classes)
            return found

        def event_of(hid):
            return next(node for node in nodes_of(hid) if "/event" in node)

        def hidden_nodes(hid):
            return sorted(node for node, (group, _) in nodes_of(hid).items() if group == "ctrlassist-hider")

        # An Xbox Series pad on Bluetooth, made with its own report
        # descriptor, which hid-microsoft binds: its share button is the
        # key KEY_RECORD (167) on its joystick node, which udev marks as
        # having a key, and as no keyboard.
        series = made("series", "Xbox Wireless Controller", "series", SERIES)
        driver = machine.succeed(f"basename $(readlink -f /sys/bus/hid/devices/{series}/driver)").strip()
        nodes = nodes_of(series)
        keys = machine.succeed(f"cat /sys/bus/hid/devices/{series}/input/input*/capabilities/key").strip()
        print(f"an Xbox Series pad on Bluetooth, the {driver} driver's: {nodes}, keys {keys}")
        assert driver == "microsoft", driver
        event = event_of(series)
        assert nodes[event] == ("input", ["ID_INPUT_JOYSTICK=1", "ID_INPUT_KEY=1"]), nodes
        assert keys == "7fff000000000000 0 8000000000 0 0", keys
        machine.succeed(f"ctrlassist-hider check {event} | grep '^pad '")
        # Its device is a pad's to the rule, which gives the group its
        # uevent file.
        machine.succeed(f"stat -c %G /sys/bus/hid/devices/{series}/uevent | grep -x ctrlassist-hider")
        # Hidden: every node of it, its raw node too.
        control, told = hide("hide-series", event)
        assert told.startswith("ok"), told
        machine.succeed("udevadm settle")
        nodes = nodes_of(series)
        print(f"a hidden Xbox Series pad: {nodes}")
        assert len(nodes) == 3 and hidden_nodes(series) == sorted(nodes), nodes
        # Plugged in while it is hidden, with its address: a twin of it,
        # and a pad of eleven keys all at 128 or above, are hidden as they
        # come, as pads, and libinput leaves the second alone, though with
        # ten keys or more it would take it for a keyboard.
        mark = reader_mark()
        twin = made("series-twin", "Xbox Wireless Controller", "series", SERIES)
        many = made("many", "Many Keys Pad", "many", MANY)
        print(f"a twin of it: {nodes_of(twin)}; a pad of eleven keys: {nodes_of(many)}")
        assert hidden_nodes(twin) == sorted(nodes_of(twin)), nodes_of(twin)
        node = event_of(many)
        assert nodes_of(many)[node][1] == ["ID_INPUT_JOYSTICK=1", "ID_INPUT_KEY=1"], nodes_of(many)
        assert [path.split("/")[-1][:2] for path in hidden_nodes(many)] == ["ev", "js"], nodes_of(many)
        view = libinput_view(node, mark, taken=False)
        print(f"a pad of eleven keys, hidden as it came: {node} {view}")
        assert view == dict(ignored=True, listed=False, took=False, held=False), view
        # An Elite Series 2 on Bluetooth, with its address too: the boot
        # keyboard's collection it carries is on its pad's node, which udev
        # takes for a keyboard alone. No marker hides it, and the service
        # takes it for no pad.
        elite = made("elite", "Xbox Wireless Controller", "elite", ELITE)
        driver = machine.succeed(f"basename $(readlink -f /sys/bus/hid/devices/{elite}/driver)").strip()
        nodes = nodes_of(elite)
        print(f"an Elite Series 2 on Bluetooth, the {driver} driver's, with a hidden pad's address: {nodes}")
        assert driver == "microsoft", driver
        event = event_of(elite)
        assert "ID_INPUT_KEYBOARD=1" in nodes[event][1] and "ID_INPUT_JOYSTICK=1" not in nodes[event][1], nodes
        assert hidden_nodes(elite) == [], nodes
        machine.succeed(f"ctrlassist-hider check {event} | grep refused")
        # The pad shown again: the sweep shows its twin and the pad of
        # eleven keys again, every node of each, as it does any pad no
        # marker names any more, and libinput takes the second for a
        # keyboard, as it would have from the start.
        mark = reader_mark()
        assert order(control, "restore").startswith("ok")
        for hid in (series, twin, many):
            machine.wait_until_succeeds(f"! ( cd /sys/bus/hid/devices/{hid} && stat -c %G input/input*/event* input/input*/js* hidraw/hidraw* 2>/dev/null | grep -qx ctrlassist-hider )")
        machine.succeed("udevadm settle")
        for hid in (series, twin, many):
            assert hidden_nodes(hid) == [] and alice_opens(event_of(hid)), nodes_of(hid)
        assert hidden_nodes(elite) == [], nodes_of(elite)
        node = event_of(many)
        view = libinput_view(node, mark, taken=True)
        print(f"a pad of eleven keys, shown again: {node} {view}")
        # Whether the compositor still holds it as it is looked at isn't
        # asked: seen held, then not, a moment apart, in one run, as the
        # sweep re-bound the twin and the pad together.
        assert {key: view[key] for key in ("ignored", "listed", "took")} == dict(ignored=False, listed=True, took=True), view
        machine.succeed("systemctl stop hide-series series series-twin many elite")
        wait_restored()

    with subtest("a start that keeps failing as a client waits is given up at systemd's limit, and the client let go"):
        # As where the package's user is missing. Stopped first, so its
        # own stop runs as it is installed.
        machine.succeed("systemctl stop ctrlassist-hider.service")
        machine.succeed("mkdir -p /run/systemd/system/ctrlassist-hider.service.d")
        machine.succeed("printf '[Service]\\nUser=no-such-user\\n' > /run/systemd/system/ctrlassist-hider.service.d/broken.conf")
        machine.succeed("systemctl daemon-reload")
        failed = "journalctl -o cat | grep -c 'ctrlassist-hider.service: Main process exited, code=exited, status=217/USER' || true"
        before = int(machine.succeed(failed))
        machine.succeed(f"systemd-run --unit stranded --uid alice ctrlassist-fixture version --socket {SOCKET}")
        machine.wait_until_succeeds("systemctl show -p Result ctrlassist-hider.socket | grep -qx Result=service-start-limit-hit")
        machine.wait_until_fails("systemctl is-active stranded")
        starts = int(machine.succeed(failed)) - before
        told = machine.succeed("journalctl -u stranded -o cat | grep -v -e '^Started' -e '^Stopped' -e 'Failed with result' -e 'Main process exited' || true").strip()
        print(f"a service that can't start: {starts} failed starts; the socket: " + machine.succeed("systemctl show -p ActiveState -p Result ctrlassist-hider.socket").replace("\n", " ") + f"; the waiting client: {told}")
        assert starts == 5, starts
        # Mended, and its socket started again by hand: served.
        machine.succeed("rm /run/systemd/system/ctrlassist-hider.service.d/broken.conf")
        machine.succeed("systemctl daemon-reload")
        machine.succeed("systemctl reset-failed ctrlassist-hider.service ctrlassist-hider.socket")
        machine.succeed("systemctl start ctrlassist-hider.socket")
        told = machine.succeed(f"su -l alice -c 'ctrlassist-fixture version --socket {SOCKET}'").strip()
        assert told == "version: ${hider.version}", told
  '';
}
