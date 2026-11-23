import re
import shlex

HIDE = "/run/ctrlassist/hide"
# Transient units on NixOS have no PATH of their own.
PATH = "/run/current-system/sw/bin"
A, B = "02:ca:7e:00:00:01", "02:ca:7e:00:00:02"
PLAIN = "Fixture Fake Pad"


def finding(spike, text):
    print(f"FINDING [{spike}] {text}")


def run(command):
    """Runs a shell command, whatever its outcome; returns its status and output."""
    status, output = machine.execute(command + " 2>&1")
    return status, output.strip()


def settle():
    machine.succeed("udevadm settle")


def pads():
    """Each pad the fixture lists, by its address or else its name."""
    found = {}
    for line in machine.succeed("ctrlassist-fixture pads").splitlines():
        uniq = re.search(r'uniq="([^"]*)"', line).group(1)
        name = re.search(r'name="([^"]*)"', line).group(1)
        found[uniq or name] = {
            "hid": re.search(r"hid=(\S+)", line).group(1),
            "nodes": {kind: node.rsplit(":", 1)[0] for kind, node in re.findall(r"(event|js|hidraw)=(\S+)", line)},
        }
    return found


def nodes(key):
    return list(pads()[key]["nodes"].values())


def opens(user, node):
    """Whether `user` can open `node` for reading."""
    return machine.execute(f"su -s /bin/sh {user} -c 'true 3<{node}'")[0] == 0


def describe(node):
    """A node's mode, group, named ACL entries, whether uaccess is a current tag, and who can open it."""
    stat = machine.succeed(f"stat -c '%a %G' {node}").split()
    acl = machine.succeed(
        f"getfacl -cp {node} 2>/dev/null | grep -E '^user:[^:]+:' | cut -d: -f2-3 | tr '\\n' ' ' || true"
    ).split()
    tags = machine.succeed(f"udevadm info -q property -n {node} | sed -n 's/^CURRENT_TAGS=//p'")
    return {
        "node": node,
        "mode": stat[0],
        "group": stat[1],
        "acl": acl,
        "uaccess": ":uaccess:" in tags,
        "alice": opens("alice", node),
        "bob": opens("bob", node),
    }


def show(state):
    return (
        f"{state['node']} mode={state['mode']} group={state['group']} acl={','.join(state['acl']) or '-'} "
        f"uaccess={'yes' if state['uaccess'] else 'no'} alice={'open' if state['alice'] else 'denied'} "
        f"bob={'open' if state['bob'] else 'denied'}"
    )


def hidden(state):
    return (
        state["group"] == "ctrlassist-hider"
        and state["mode"] == "660"
        and not state["acl"]
        and not state["uaccess"]
        and not state["alice"]
        and not state["bob"]
    )


def report(spike, what, node_list):
    states = [describe(node) for node in node_list]
    for state in states:
        finding(spike, f"{what}: {show(state)}")
    return states


def service(command, user=None, properties=(), unit=None):
    """Runs a command as a transient service, as `user` and with these unit properties; returns its output."""
    arguments = ["systemd-run", "--wait", "--pipe", "--quiet", "--collect", f"--setenv=PATH={PATH}"]
    if unit:
        arguments += ["--unit", unit]
    if user:
        # The user's own primary group, as User= gives.
        arguments += ["--uid", user]
    for prop in properties:
        arguments += ["-p", prop]
    _, output = run(shlex.join(arguments + command) + " </dev/null")
    return output


def fake(unit, *arguments):
    machine.succeed(f"systemd-run --unit {unit} ctrlassist-fixture " + shlex.join(arguments))


def rebind_root(bus, driver, device):
    machine.succeed(f"echo {device} > /sys/bus/{bus}/drivers/{driver}/unbind")
    machine.succeed(f"echo {device} > /sys/bus/{bus}/drivers/{driver}/bind")
    settle()


def listen(unit, user, node):
    """Starts a reader on `node` as `user` and waits until it is reading."""
    machine.succeed(f"systemd-run --unit {unit} --uid {user} ctrlassist-fixture listen {node} --millis 60000")
    machine.wait_until_succeeds(f"journalctl -u {unit} -o cat | grep -c >/dev/null 't=1s'")


def was_cut_off(unit):
    machine.wait_until_succeeds(f"journalctl -u {unit} -o cat | grep -c >/dev/null 'cut_off='", timeout=70)
    line = machine.succeed(f"journalctl -u {unit} -o cat | grep 'cut_off='").strip()
    return "cut_off=true" in line, line


def tablet():
    """The event node of the tablet with the serial number the VM gave it."""
    return machine.succeed("readlink -f /dev/input/by-id/*CTRLSPIKE1*event*").split()[0]


def active_user():
    return machine.succeed(
        "loginctl show-session $(loginctl show-seat seat0 -p ActiveSession --value) -p Name --value"
    ).strip()


start_all()
machine.wait_for_unit("multi-user.target")
machine.wait_for_file("/run/user/1000/wayland-0.lock")
settle()

kernel = machine.succeed("uname -r").strip()
udev = machine.succeed("udevadm --version").strip()
finding("versions", f"kernel {kernel}, systemd/udev {udev}")
finding("versions", "udev(7) on TEST: " + " ".join(machine.succeed(
    "man -P cat udev 2>/dev/null | grep -A4 -E '^ +TEST' | tr -s ' \\n' '  ' || true"
).split()))

# --- Spike 3a, at boot and when a module loads later ---------------------

with subtest("3a: driver files at boot and when a module loads later"):
    for line in machine.succeed(
        "stat -c '%A %U:%G %n' /sys/bus/hid/drivers/playstation/bind /sys/bus/hid/drivers/playstation/unbind "
        "/sys/bus/usb/drivers/usbhid/bind /sys/bus/usb/drivers/usbhid/unbind"
    ).splitlines():
        finding("3a", f"at boot: {line}")
    loaded = machine.execute("test -d /sys/bus/usb/drivers/xpad")[0] == 0
    finding("3a", f"xpad loaded at boot: {loaded}")
    machine.succeed("modprobe xpad")
    settle()
    for line in machine.succeed("stat -c '%A %U:%G %n' /sys/bus/usb/drivers/xpad/bind /sys/bus/usb/drivers/xpad/unbind").splitlines():
        finding("3a", f"after modprobe xpad: {line}")
    machine.succeed("rmmod hid_playstation && modprobe hid_playstation")
    settle()
    for line in machine.succeed("stat -c '%A %U:%G %n' /sys/bus/hid/drivers/playstation/bind").splitlines():
        finding("3a", f"after reloading hid_playstation: {line}")

# --- Spike 1: the marker rule ------------------------------------------

fake("ds1", "fake-dualsense", "--real", "--number", "1", "--pulse", "20")
fake("ds2", "fake-dualsense", "--real", "--number", "2", "--pulse", "20")
machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -c >/dev/null {A}")
machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -c >/dev/null {B}")
settle()

with subtest("1: a DualSense with an address, hidden by marker on the add after a re-bind"):
    finding("1", f"rules in /run/udev/rules.d: [{run('ls /run/udev/rules.d')[1]}]")
    report("1", "DualSense A before", nodes(A))
    machine.succeed(f"touch {HIDE}/addr-{A}")
    settle()
    finding("1", "marker created, no event yet: " + show(describe(pads()[A]["nodes"]["event"])))
    rebind_root("hid", "playstation", pads()[A]["hid"])
    states = report("1", "DualSense A after marker + re-bind", nodes(A))
    finding("1", f"A hidden on re-bind: {all(hidden(s) for s in states)} ({len(states)} nodes)")
    others = report("1", "DualSense B (no marker)", nodes(B))
    finding("1", f"B untouched: {not any(hidden(s) for s in others)}")

    machine.succeed("systemctl restart ds1")
    machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -c >/dev/null {A}")
    settle()
    states = report("1", "DualSense A after reconnect", nodes(A))
    finding("1", f"A hidden on reconnect: {all(hidden(s) for s in states)}")

    machine.succeed(f"rm {HIDE}/addr-{A}")
    rebind_root("hid", "playstation", pads()[A]["hid"])
    states = report("1", "DualSense A after marker deleted + re-bind", nodes(A))
    finding("1", f"A restored: {all(s['alice'] and 'alice' in ' '.join(s['acl']) and s['uaccess'] for s in states)}")
    finding("1", f"rules in /run/udev/rules.d: [{run('ls /run/udev/rules.d')[1]}]")
    status, log = run("journalctl -u systemd-udevd -o cat | grep -iE 'reload|rules' | tail -5")
    finding("1", f"udev's log on rules: [{log}]")

with subtest("1: the hidraw line of the rule the service writes today"):
    # udev sorts a line's tokens by type, and ENV matches come before
    # IMPORT{parent}: on one line, the address is compared before it is
    # imported. Named 71- so that it runs before the static rule, whose
    # own IMPORT{parent} would otherwise have brought the address in.
    prefix = pads()[A]["hid"][:14]
    today = (
        f'SUBSYSTEM=="hidraw", KERNELS=="{prefix}.*", IMPORT{{parent}}="HID_UNIQ", '
        f'ENV{{HID_UNIQ}}=="{A}", MODE="0000", GROUP="root", TAG-="uaccess"'
    )
    machine.succeed(f"mkdir -p /run/udev/rules.d && echo '{today}' > /run/udev/rules.d/71-ctrlassist-today.rules")
    machine.succeed("udevadm control --reload")
    rebind_root("hid", "playstation", pads()[A]["hid"])
    state = report("1", "today's hidraw line, one line", [pads()[A]["nodes"]["hidraw"]])[0]
    finding("1", f"today's one-line hidraw rule denies the node: {state['mode'] == '0' and not state['alice']}")
    split = (
        f'SUBSYSTEM=="hidraw", KERNELS=="{prefix}.*", IMPORT{{parent}}="HID_UNIQ"\n'
        f'SUBSYSTEM=="hidraw", KERNELS=="{prefix}.*", ENV{{HID_UNIQ}}=="{A}", MODE="0000", GROUP="root", TAG-="uaccess"'
    )
    machine.succeed(f"printf '%s\\n' '{split}' > /run/udev/rules.d/71-ctrlassist-today.rules")
    machine.succeed("udevadm control --reload")
    rebind_root("hid", "playstation", pads()[A]["hid"])
    state = report("1", "today's hidraw line, split in two", [pads()[A]["nodes"]["hidraw"]])[0]
    finding("1", f"split in two, it denies the node: {state['mode'] == '0' and not state['alice']}")
    machine.succeed("rm /run/udev/rules.d/71-ctrlassist-today.rules && udevadm control --reload")
    rebind_root("hid", "playstation", pads()[A]["hid"])

with subtest("1: a uinput pad with no address, hidden by model"):
    fake("plain", "fake-pad", "--pulse", "20")
    machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -c >/dev/null '{PLAIN}'")
    settle()
    report("1", "uinput pad before", nodes(PLAIN))
    machine.succeed(f"touch {HIDE}/model-1209-5eed")
    for node in nodes(PLAIN):
        machine.succeed(f"udevadm trigger --action=change --settle {node}")
    states = report("1", "uinput pad after marker + change event", nodes(PLAIN))
    finding("1", f"uinput pad hidden by a change event: {all(hidden(s) for s in states)}")
    machine.succeed("systemctl restart plain")
    machine.wait_until_succeeds(f"ctrlassist-fixture pads | grep -c >/dev/null '{PLAIN}'")
    settle()
    states = report("1", "uinput pad after reconnect", nodes(PLAIN))
    finding("1", f"uinput pad hidden on reconnect: {all(hidden(s) for s in states)}")
    machine.succeed(f"rm {HIDE}/model-1209-5eed")
    for node in nodes(PLAIN):
        machine.succeed(f"udevadm trigger --action=change --settle {node}")
    states = report("1", "uinput pad after marker deleted + change event", nodes(PLAIN))
    finding("1", f"uinput pad restored by a change event: {all(s['alice'] for s in states)}")
    machine.succeed("systemctl stop plain")

TABLET = tablet()
TABLET_IFACE = machine.succeed(f"basename $(readlink -f /sys/class/input/{TABLET.split('/')[-1]}/device/device/..)").strip()

with subtest("1: a USB device known by its serial number, re-bound on the USB bus"):
    props = machine.succeed(
        f"udevadm info -q property -n {TABLET} | grep -E '^(ID_SERIAL_SHORT|ID_VENDOR_ID|ID_MODEL_ID)='"
    ).split()
    uniq = machine.succeed(f"cat /sys/class/input/{TABLET.split('/')[-1]}/device/uniq").strip()
    finding("1", f"tablet {TABLET} on interface {TABLET_IFACE}: {' '.join(props)} uniq={uniq!r}")
    report("1", "tablet before", [TABLET])
    machine.succeed(f"touch {HIDE}/serial-0627-0001-CTRLSPIKE1")
    rebind_root("usb", "usbhid", TABLET_IFACE)
    TABLET = tablet()
    state = report("1", "tablet after serial marker + re-bind", [TABLET])[0]
    finding("1", f"tablet given to the group by serial: {state['group'] == 'ctrlassist-hider' and state['mode'] == '660'}")
    machine.succeed(f"rm {HIDE}/serial-0627-0001-CTRLSPIKE1")
    rebind_root("usb", "usbhid", TABLET_IFACE)
    TABLET = tablet()
    report("1", "tablet after marker deleted + re-bind", [TABLET])

with subtest("1: hostile attribute values"):
    # A pad whose address is a path: with no marker it must not be hidden,
    # and a marker it names by traversal must not exist.
    fake("evil", "fake-hid", "--name", "Evil Pad", "--uniq", "/../../../etc/passwd", "--product", "0bad", "--pulse", "20")
    machine.wait_until_succeeds("ctrlassist-fixture pads | grep -c >/dev/null 'Evil Pad'")
    settle()
    evil = pads()["/../../../etc/passwd"]
    state = report("1", "pad with uniq '/../../../etc/passwd'", [evil["nodes"]["event"]])[0]
    finding("1", f"traversal address hidden: {state['group'] == 'ctrlassist-hider'}")
    machine.succeed("systemctl stop evil")

# --- Spike 2: a group instead of mode 0000 ------------------------------

with subtest("2: a seat user in the input group, and the service's own user"):
    finding("2", "alice: " + machine.succeed("id alice").strip())
    machine.succeed(f"touch {HIDE}/addr-{A}")
    rebind_root("hid", "playstation", pads()[A]["hid"])
    states = report("2", "A hidden, alice in input", nodes(A))
    finding("2", f"alice (input member) can open a hidden node: {any(s['alice'] for s in states)}")

    a_nodes = pads()[A]["nodes"]
    b_nodes = pads()[B]["nodes"]
    for kind in ["event", "hidraw"]:
        machine.succeed("rm -f /tmp/fdpass/*.done")
        machine.succeed(f"systemd-run --unit recv-{kind} --uid alice fdpass recv /tmp/fdpass/{kind}.sock")
        machine.wait_until_succeeds(f"journalctl -u recv-{kind} -o cat | grep -c >/dev/null listening")
        sent = service(
            ["fdpass", "send", a_nodes[kind], f"/tmp/fdpass/{kind}.sock"],
            user="ctrlassist-hider",
            properties=["CapabilityBoundingSet=", "AmbientCapabilities=", "NoNewPrivileges=yes"],
        )
        finding("2", f"service user sends hidden {kind}: {' | '.join(sent.splitlines())}")
        machine.wait_until_succeeds(f"journalctl -u recv-{kind} -o cat | grep -c >/dev/null 'reads='")
        finding("2", f"alice receives: {machine.succeed(f'journalctl -u recv-{kind} -o cat | grep reads=').strip()}")

    for kind in ["event", "hidraw"]:
        out = service(["spike-hide", "open", b_nodes[kind]], user="ctrlassist-hider", properties=["CapabilityBoundingSet="])
        finding("2", f"service user opens B's {kind}, not hidden: {out.splitlines()[-1]}")

with subtest("2: a VT switch and a user switch"):
    report("2", "before switching, A", nodes(A))
    report("2", "before switching, B", nodes(B))
    finding("2", f"active: {active_user()}")
    machine.succeed("chvt 2")
    machine.wait_until_tty_matches("2", "login: ")
    machine.send_chars("bob\n")
    machine.wait_until_tty_matches("2", "Password: ")
    machine.send_chars("bob\n")
    machine.wait_until_succeeds("[ $(loginctl show-session $(loginctl show-seat seat0 -p ActiveSession --value) -p Name --value) = bob ]")
    settle()
    finding("2", f"active: {active_user()}")
    a = report("2", "bob's session in front, A", nodes(A))
    report("2", "bob's session in front, B", nodes(B))
    machine.succeed("chvt 1")
    machine.wait_until_succeeds("[ $(loginctl show-session $(loginctl show-seat seat0 -p ActiveSession --value) -p Name --value) = alice ]")
    settle()
    finding("2", f"active: {active_user()}")
    back = report("2", "alice's session in front again, A", nodes(A))
    report("2", "alice's session in front again, B", nodes(B))
    finding("2", f"an ACL came back on a hidden node: {any(s['acl'] for s in a + back)}")
    machine.succeed(f"rm {HIDE}/addr-{A}")
    rebind_root("hid", "playstation", pads()[A]["hid"])

# --- Spike 3a: re-binding as a member of the group ----------------------

NO_CAPS = ["CapabilityBoundingSet=", "AmbientCapabilities=", "NoNewPrivileges=yes"]

with subtest("3a: the service's user re-binds and cuts off a reader"):
    hid_b = pads()[B]["hid"]
    out = service(["spike-hide", "hide", f"addr-{B}", "hid", "playstation", hid_b], user="bob", properties=NO_CAPS)
    finding("3a", f"bob (not in the group) hides B: {' | '.join(out.splitlines())}")
    run(f"rm -f {HIDE}/addr-{B}")
    listen("early", "alice", pads()[B]["nodes"]["hidraw"])
    listen("early-ev", "alice", pads()[B]["nodes"]["event"])
    out = service(["spike-hide", "hide", f"addr-{B}", "hid", "playstation", hid_b], user="ctrlassist-hider", properties=NO_CAPS)
    finding("3a", f"service user hides B: {' | '.join(out.splitlines())}")
    settle()
    for unit in ["early", "early-ev"]:
        cut, line = was_cut_off(unit)
        finding("3a", f"alice's reader ({unit}) cut off: {cut} [{line}]")
    states = report("3a", "B after the service user's re-bind", nodes(B))
    finding("3a", f"B hidden: {all(hidden(s) for s in states)}")
    out = service(["spike-hide", "restore", f"addr-{B}", "hid", "playstation", hid_b], user="ctrlassist-hider", properties=NO_CAPS)
    finding("3a", f"service user restores B: {' | '.join(out.splitlines())}")
    settle()
    states = report("3a", "B after restore", nodes(B))

with subtest("3a: the service's user re-binds on the USB bus"):
    out = service(["spike-hide", "hide", "serial-0627-0001-CTRLSPIKE1", "usb", "usbhid", TABLET_IFACE], user="ctrlassist-hider", properties=NO_CAPS)
    finding("3a", f"service user re-binds the tablet's usbhid: {' | '.join(out.splitlines())}")
    settle()
    TABLET = tablet()
    report("3a", "tablet after", [TABLET])
    out = service(["spike-hide", "restore", "serial-0627-0001-CTRLSPIKE1", "usb", "usbhid", TABLET_IFACE], user="ctrlassist-hider", properties=NO_CAPS)
    settle()

with subtest("3a: an Xbox 360 pad under xpad, made with a USB gadget"):
    status, out = run("modprobe dummy_hcd && modprobe libcomposite && modprobe usb_f_fs")
    finding("3a", f"dummy_hcd, libcomposite, usb_f_fs: {'loaded' if status == 0 else 'unavailable: ' + out}")
    if status == 0:
        machine.succeed(f"systemd-run --unit xpad-gadget --setenv=PATH={PATH} spike-xpad XPADSPIKE1")
        status, _ = machine.execute("timeout 30 sh -c 'until ctrlassist-fixture pads | grep -c >/dev/null X-Box; do sleep 0.5; done'")
        finding("3a", "gadget log: " + " | ".join(machine.succeed("journalctl -u xpad-gadget -o cat | head -8").splitlines()))
        if status == 0:
            settle()
            key = [k for k in pads() if "X-Box" in k][0]
            event = pads()[key]["nodes"]["event"]
            sysname = event.split("/")[-1]
            iface = machine.succeed(f"basename $(readlink -f /sys/class/input/{sysname}/device/device)").strip()
            driver = machine.succeed(f"basename $(readlink -f /sys/class/input/{sysname}/device/device/driver)").strip()
            serial = machine.succeed(f"udevadm info -q property -n {event} | sed -n 's/^ID_SERIAL_SHORT=//p'").strip()
            uniq = machine.succeed(f"cat /sys/class/input/{sysname}/device/uniq").strip()
            finding("3a", f"xpad pad {key!r} on {iface} driver={driver} ID_SERIAL_SHORT={serial} uniq={uniq!r}")
            report("3a", "xpad pad before", nodes(key))
            listen("early-x", "alice", event)
            marker = f"serial-045e-028e-{serial}"
            out = service(["spike-hide", "hide", marker, "usb", "xpad", iface], user="ctrlassist-hider", properties=NO_CAPS)
            finding("3a", f"service user hides the xpad pad: {' | '.join(out.splitlines())}")
            settle()
            cut, line = was_cut_off("early-x")
            finding("3a", f"alice's reader on the xpad pad cut off: {cut} [{line}]")
            states = report("3a", "xpad pad after", nodes(key))
            finding("3a", f"xpad pad hidden by serial: {all(hidden(s) for s in states)}")
            out = service(["spike-hide", "restore", marker, "usb", "xpad", iface], user="ctrlassist-hider", properties=NO_CAPS)
            finding("3a", f"service user restores the xpad pad: {' | '.join(out.splitlines())}")
            settle()
            report("3a", "xpad pad after restore", nodes(key))
        else:
            finding("3a", "xpad pad didn't appear: " + run("lsusb; dmesg | tail -20")[1].replace("\n", " | "))

# --- Spike 3b: re-binding through a synthetic event ---------------------

with subtest("3b: a synthetic uevent the service's user sends, and a rule that re-binds"):
    hid_b = pads()[B]["hid"]
    uevent = f"/sys/bus/hid/devices/{hid_b}/uevent"
    finding("3b", "uevent file: " + machine.succeed(f"stat -c '%A %U:%G' {uevent}").strip())
    listen("early-3b", "alice", pads()[B]["nodes"]["hidraw"])
    machine.succeed(f"touch {HIDE}/addr-{B}")
    machine.succeed(f"chgrp ctrlassist-hider {HIDE}/addr-{B}")
    before = machine.succeed("date +%s.%N").strip()
    out = service(
        ["sh", "-c", f"echo \"change $(cat /proc/sys/kernel/random/uuid) CTRLASSIST=rebind\" > {uevent} && echo sent"],
        user="ctrlassist-hider",
        properties=NO_CAPS,
    )
    finding("3b", f"service user writes the uevent: {out}")
    status, _ = run("udevadm settle --timeout=60")
    after = machine.succeed("date +%s.%N").strip()
    finding("3b", f"udev settled: {status == 0}, in {float(after) - float(before):.2f}s")
    cut, line = was_cut_off("early-3b")
    finding("3b", f"alice's reader cut off: {cut} [{line}]")
    states = report("3b", "B after the synthetic re-bind", nodes(B))
    finding("3b", f"B hidden: {all(hidden(s) for s in states)}")
    finding("3b", "udev's log: " + run("journalctl -u systemd-udevd -o cat | grep -iE 'timeout|timed out|kill|fail|error' | tail -5")[1].replace("\n", " | "))
    machine.succeed(f"rm {HIDE}/addr-{B}")
    out = service(
        ["sh", "-c", f"echo \"change $(cat /proc/sys/kernel/random/uuid) CTRLASSIST=rebind\" > {uevent} && echo sent"],
        user="ctrlassist-hider",
        properties=NO_CAPS,
    )
    settle()
    states = report("3b", "B after marker deleted + synthetic re-bind", nodes(B))
    finding("3b", f"B restored: {all(s['alice'] for s in states)}")

# --- Spike 4: a shrunk root --------------------------------------------

PACKAGED = [
    "NoNewPrivileges=yes",
    "ProtectSystem=strict",
    "ProtectHome=yes",
    "PrivateTmp=yes",
    "RestrictAddressFamilies=AF_UNIX AF_NETLINK",
    "ProtectKernelModules=yes",
    "ProtectKernelLogs=yes",
    "ProtectControlGroups=yes",
    "ProtectClock=yes",
    "ProtectHostname=yes",
    "RestrictNamespaces=yes",
    "RestrictRealtime=yes",
    "RestrictSUIDSGID=yes",
    "LockPersonality=yes",
    "MemoryDenyWriteExecute=yes",
    "SystemCallArchitectures=native",
    "SystemCallFilter=@system-service",
]
SHRUNK_ONLY = [
    "CapabilityBoundingSet=",
    "AmbientCapabilities=",
    "ProtectKernelTunables=yes",
    f"ReadWritePaths={HIDE}",
    "ReadWritePaths=/sys/bus/hid/drivers/playstation",
    "ReadWritePaths=/sys/bus/usb/drivers/usbhid",
    "DevicePolicy=closed",
    "DeviceAllow=char-input rw",
    "DeviceAllow=char-hidraw rw",
    "ProtectProc=invisible",
]
SHRUNK = PACKAGED + SHRUNK_ONLY
# What works on systemd 262 instead (see sandbox.nix). ProtectKernelTunables=,
# ProtectControlGroups= and ProtectProc= each imply MountAPIVFS=, whose /run and
# /sys entries leave both writable under ProtectSystem=strict; naming them
# read-only with entries that outrank those puts that right. And no AF_UNIX
# sockets of its own, so that it can't reach the service manager.
FIXED = [p for p in SHRUNK if not p.startswith("RestrictAddressFamilies")] + [
    "ReadOnlyPaths=/run",
    "BindReadOnlyPaths=/sys",
    "RestrictAddressFamilies=AF_NETLINK",
]


def root_hide_restore(name, properties):
    """Hides B and restores it as root under `properties`; reports each step."""
    hid_b = pads()[B]["hid"]
    out = service(["spike-hide", "hide", f"addr-{B}", "hid", "playstation", hid_b], properties=properties)
    finding("4", f"{name}: hide: {' | '.join(out.splitlines())}")
    settle()
    states = report("4", f"{name}: B after hide", nodes(B))
    out = service(["spike-hide", "open"] + nodes(B), properties=properties)
    finding("4", f"{name}: open: {' | '.join(out.splitlines()[1:])}")
    out = service(["spike-hide", "restore", f"addr-{B}", "hid", "playstation", hid_b], properties=properties)
    finding("4", f"{name}: restore: {' | '.join(out.splitlines())}")
    settle()
    restored = report("4", f"{name}: B after restore", nodes(B))
    finding("4", f"{name}: hid={all(hidden(s) for s in states)} restored={all(s['alice'] for s in restored)}")
    run(f"rm -f {HIDE}/addr-{B}")
    rebind_root("hid", "playstation", pads()[B]["hid"])


def escaped():
    """Whether spike-escaped ran; a job started over the bus runs after the call returns."""
    return machine.execute("timeout 10 sh -c 'until test -e /run/escaped; do sleep 0.2; done'")[0] == 0


def reach(name, properties):
    """What else a root with no capabilities can do under `properties`."""
    out = service(["sh", "-c", "grep -E ' (/run|/sys) ' /proc/self/mountinfo | cut -d' ' -f5,6 | tr '\\n' ';'"], properties=properties)
    finding("4", f"{name}: /run and /sys as mounted: {out}")
    finding("4", f"{name}: seat file: " + service(["sh", "-c", "sed -n 2p /run/systemd/seats/seat0"], properties=properties))
    out = service(["sh", "-c", "mkdir -p /run/udev/rules.d && echo '# x' > /run/udev/rules.d/99-x.rules && echo wrote"], properties=properties)
    finding("4", f"{name}: writes a runtime udev rule: {out}")
    run("rm -f /run/udev/rules.d/99-x.rules")
    out = service(["sh", "-c", "touch /etc/x && echo wrote"], properties=properties)
    finding("4", f"{name}: writes /etc: {out}")
    out = service(["sh", "-c", "cat /proc/sys/kernel/sysrq > /proc/sys/kernel/sysrq && echo wrote"], properties=properties)
    finding("4", f"{name}: writes /proc/sys/kernel/sysrq: {out}")
    udevd = machine.succeed("systemctl show -p MainPID --value systemd-udevd").strip()
    out = service(["sh", "-c", f"kill -0 {udevd} && echo may signal udevd"], properties=properties)
    finding("4", f"{name}: signals udevd (pid {udevd}): {out}")
    out = service(["spike-hide", "probe", "/sys/bus/hid/drivers/hid-generic/bind", "/sys/bus/hid/drivers/playstation/new_id"], properties=properties)
    finding("4", f"{name}: writes other sysfs files: {' | '.join(out.splitlines()[1:])}")
    for how, command in [
        ("systemctl (private socket)", ["systemctl", "start", "spike-escaped.service"]),
        ("busctl (system bus)", ["busctl", "call", "org.freedesktop.systemd1", "/org/freedesktop/systemd1",
                                 "org.freedesktop.systemd1.Manager", "StartUnit", "ss", "spike-escaped.service", "replace"]),
    ]:
        run("rm -f /run/escaped")
        out = service(command, properties=properties)
        finding("4", f"{name}: starts a unit through {how}: escaped={escaped()} [{out}]")
    run("rm -f /run/escaped")


with subtest("4: hide and restore as root with no capabilities"):
    root_hide_restore("shrunk", SHRUNK)
    root_hide_restore("shrunk without the sysfs paths", [p for p in SHRUNK if not p.startswith("ReadWritePaths=/sys")])
    root_hide_restore("shrunk without char-hidraw", [p for p in SHRUNK if p != "DeviceAllow=char-hidraw rw"])
    root_hide_restore("shrunk without ProtectProc", [p for p in SHRUNK if not p.startswith("ProtectProc")])
    root_hide_restore("fixed", FIXED)
    root_hide_restore("fixed without the sysfs paths", [p for p in FIXED if not p.startswith("ReadWritePaths=/sys")])

with subtest("4: a node of mode 0000, as the runtime rule makes them"):
    node = pads()[B]["nodes"]["event"]
    machine.succeed(f"chmod 0000 {node}")
    out = service(["spike-hide", "open", node], properties=SHRUNK)
    finding("4", f"shrunk root opens a mode-0000 node: {out.splitlines()[-1]}")
    # A change event doesn't put the mode back.
    machine.succeed(f"chmod 0660 {node}")

with subtest("4: what else a root with no capabilities can do"):
    reach("shrunk", SHRUNK)
    reach("fixed", FIXED)

with subtest("4: a driver that loads after the service starts"):
    status, out = run("systemctl stop xpad-gadget; rmmod xpad")
    finding("4", f"xpad unloaded: {status == 0} {out}")
    late = FIXED + ["ReadWritePaths=-/sys/bus/usb/drivers/xpad"]
    arguments = ["systemd-run", "--unit", "late", "--collect", f"--setenv=PATH={PATH}"] + [a for p in late for a in ("-p", p)]
    machine.succeed(shlex.join(arguments + ["sh", "-c", "while [ ! -e /run/ctrlassist/hide/go ]; do sleep 0.2; done; spike-hide probe /sys/bus/usb/drivers/xpad/bind"]))
    machine.succeed("modprobe xpad")
    settle()
    machine.succeed(f"touch {HIDE}/go")
    machine.wait_until_succeeds("journalctl -u late -o cat | grep -c >/dev/null 'write /sys'")
    finding("4", "fixed: write to xpad's bind, loaded after the service started: " + machine.succeed("journalctl -u late -o cat | grep 'write /sys'").strip())
    machine.succeed(f"rm {HIDE}/go")
