# Opens a node and passes the descriptor to another process, as the hiding
# service does for the daemon.
#
#   fdpass.py recv <socket>         wait for one descriptor, then count reads on it
#   fdpass.py send <node> <socket>  open <node> read-write and send it
import os
import select
import socket
import sys
import time


def capabilities():
    with open("/proc/self/status") as status:
        return " ".join(
            line.split()[0] + line.split()[1]
            for line in status
            if line.startswith(("CapEff", "CapPrm", "CapBnd", "CapAmb"))
        )


def send(node, path):
    print(f"sender uid={os.getuid()} gid={os.getgid()} groups={os.getgroups()} {capabilities()}")
    try:
        fd = os.open(node, os.O_RDWR | os.O_NONBLOCK | os.O_CLOEXEC)
    except OSError as error:
        print(f"open {node}: failed: {error.strerror}")
        sys.exit(1)
    print(f"open {node}: ok")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as peer:
        peer.connect(path)
        socket.send_fds(peer, [b"fd"], [fd])
    print("sent")


def receive(path):
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
        server.bind(path)
        os.chmod(path, 0o666)
        server.listen(1)
        print("listening", flush=True)
        peer, _ = server.accept()
        with peer:
            _, fds, _, _ = socket.recv_fds(peer, 16, 1)
    fd = fds[0]
    target = os.readlink(f"/proc/self/fd/{fd}")
    reads = 0
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.2)
        if ready:
            try:
                if os.read(fd, 4096):
                    reads += 1
            except BlockingIOError:
                pass
    print(f"receiver uid={os.getuid()} got {target} reads={reads}", flush=True)


if __name__ == "__main__":
    match sys.argv[1:]:
        case ["send", node, path]:
            send(node, path)
        case ["recv", path]:
            receive(path)
        case _:
            sys.exit("usage: fdpass.py send <node> <socket> | recv <socket>")
