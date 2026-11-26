# The hiding service on a kernel from before Linux 6.16, which can't
# refuse descriptors to a connection not yet admitted: the socket unit's
# setting does nothing there, and the closer is the only defense against
# a close that blocks. Checks that the service still starts and hides,
# says which it is, and closes such a connection off its main thread. The
# rest of what the service does is the `hider` check's.
{
  pkgs,
  fixture,
  hider,
}:
let
  # A user with no seat's eight connections, then a ninth that passes a
  # TCP socket whose close lingers, its data never read, before it is
  # accepted; says whether it could, how many it holds and whether the
  # ninth was closed; holds.
  lingering = pkgs.writers.writePython3Bin "lingering" { doCheck = false; } ''
    import socket
    import struct
    import sys
    import time

    path, count = sys.argv[1], int(sys.argv[2])


    def connect():
        peer = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        peer.connect(path)
        return peer


    peers = [connect() for _ in range(count)]
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
    last = connect()
    try:
        socket.send_fds(last, [struct.pack("<BI", 6, 0)], [stuck.fileno()])
        passed = "yes"
    except OSError as error:
        passed = error.strerror
        stuck.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
    stuck.close()
    try:
        closed = last.recv(6) == b""
    except ConnectionResetError:
        closed = True
    print(f"passed={passed} held={len(peers)} closed={closed}", flush=True)
    time.sleep(3600)
  '';
in
pkgs.testers.runNixOSTest {
  name = "ctrlassist-hider-old-kernel";

  nodes.machine = {
    # The last long-term kernel before SO_PASSRIGHTS.
    boot.kernelPackages = pkgs.linuxPackages_6_12;
    boot.kernelModules = [ "uhid" ];
    users.users.alice = {
      isNormalUser = true;
      uid = 1000;
    };
    users.users.bob = {
      isNormalUser = true;
      uid = 1001;
    };
    # A graphical session for alice on the seat, as the service serves
    # only the user in front of the machine.
    services.cage = {
      enable = true;
      user = "alice";
      program = "${pkgs.coreutils}/bin/sleep infinity";
    };
    virtualisation.qemu.options = [ "-vga none -device virtio-gpu-pci" ];

    systemd.packages = [ hider ];
    systemd.sockets.ctrlassist-hider.wantedBy = [ "sockets.target" ];
    users.users.ctrlassist-hider = {
      isSystemUser = true;
      group = "ctrlassist-hider";
    };
    users.groups.ctrlassist-hider = { };
    services.udev.packages = [ hider ];

    environment.systemPackages = [
      fixture
      lingering
    ];
  };

  testScript = ''
    import re

    SOCKET = "/run/ctrlassist/hider.sock"

    def hide(unit, node):
        """Starts a client of alice's that hides `node` and holds; returns its control directory and what it was told."""
        control = f"/tmp/{unit}"
        machine.succeed(
            f"systemd-run --unit {unit} --uid alice ctrlassist-fixture hide {node} --socket {SOCKET} --control {control}"
        )
        machine.wait_for_file(f"{control}/hide.done")
        return control, machine.succeed(f"cat {control}/hide.done")

    def order(control, what):
        machine.succeed(f"rm -f {control}/{what}.done; touch {control}/{what}")
        machine.wait_for_file(f"{control}/{what}.done")
        return machine.succeed(f"cat {control}/{what}.done")

    def wait_restored():
        machine.wait_until_succeeds("test -z \"$(ls -A /run/ctrlassist/hide)\"")
        machine.succeed("udevadm settle")

    machine.wait_for_file("/run/user/1000/wayland-0.lock")
    machine.wait_for_unit("sockets.target")
    print("versions: kernel " + machine.succeed("uname -r").strip() + ", " + machine.succeed("systemctl --version | head -1").strip())
    machine.succeed("systemctl is-active ctrlassist-hider.socket")
    machine.succeed("systemd-run --unit pad ctrlassist-fixture fake-hid --pulse 20")
    machine.wait_until_succeeds("ctrlassist-fixture pads | grep -q 'Fixture Fake HID Pad'")
    machine.succeed("udevadm settle")
    line = machine.succeed("ctrlassist-fixture pads | grep 'Fixture Fake HID Pad'")
    found = re.search(r"event=(\S+):", line)
    assert found, line
    node = found.group(1)

    with subtest("the service starts with the socket unit's setting unknown to the kernel, says so, and hides a pad"):
        told = machine.succeed(f"su -l alice -c 'ctrlassist-fixture version --socket {SOCKET}'").strip()
        assert told == "version: ${hider.version}", told
        machine.succeed("journalctl -u ctrlassist-hider.service -o cat | grep -q \"this kernel can't refuse descriptors\"")
        print("the socket: " + machine.succeed("systemctl show -p ActiveState -p Result ctrlassist-hider.socket").replace("\n", " ") + "; systemd said: " + machine.succeed("journalctl -b -o cat | grep -i -e passrights -e AcceptFileDescriptors || true").replace("\n", " | "))
        control, told = hide("held", node)
        assert told.startswith("ok"), told
        assert order(control, "restore").startswith("ok")
        machine.succeed("systemctl stop held")
        wait_restored()

    with subtest("a connection over the cap that carries a socket whose close lingers is closed off the main thread, and the user at the seat is served at once"):
        # Sent as the service starts, so the service's end of the ninth
        # is the last to hold the socket.
        machine.succeed("systemctl stop ctrlassist-hider.service")
        machine.succeed(f"systemd-run --unit lingering --uid bob lingering {SOCKET} 8")
        machine.wait_until_succeeds("journalctl -u lingering -o cat | grep -q held=")
        told = machine.succeed("journalctl -u lingering -o cat | grep held=").strip()
        assert told == "passed=yes held=8 closed=True", told
        pid = machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip()
        closer = f"for task in /proc/{pid}/task/*; do if grep -qx closer $task/comm; then cat $task/stack; fi; done"
        machine.wait_until_succeeds(f"{closer} | grep -q tcp_close")
        told = machine.succeed(f"su -l alice -c 'ctrlassist-fixture version --socket {SOCKET}'").strip()
        assert told == "version: ${hider.version}", told
        control, told = hide("past-lingering", node)
        assert told.startswith("ok"), told
        assert order(control, "restore").startswith("ok")
        assert machine.succeed("systemctl show -p MainPID --value ctrlassist-hider.service").strip() == pid, "the service didn't restart"
        machine.succeed("systemctl stop past-lingering lingering")
        machine.wait_until_fails(f"{closer} | grep -q tcp_close")
        wait_restored()
  '';
}
