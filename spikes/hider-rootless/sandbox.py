import shlex

PATH = "/run/current-system/sw/bin"
PROBE = (
    "for f in /run/x /run/ctrlassist/hide/x /sys/bus/hid/drivers/playstation/bind /sys/bus/hid/drivers/hid-generic/bind; do "
    "e=$( (echo nosuchdevice > $f) 2>&1 ) && echo \"$f: written\" || echo \"$f: ${e##*: }\"; done; "
    "grep -E ' (/|/run|/sys) ' /proc/self/mountinfo | cut -d' ' -f5,6 | tr '\\n' ';'"
)


def probe(name, properties):
    arguments = ["systemd-run", "--wait", "--pipe", "--quiet", "--collect", f"--setenv=PATH={PATH}"]
    for prop in properties:
        arguments += ["-p", prop]
    _, out = machine.execute(shlex.join(arguments + ["sh", "-c", PROBE]) + " </dev/null 2>&1")
    print(f"FINDING [4-sandbox] {name}: " + " | ".join(out.strip().splitlines()))


start_all()
machine.wait_for_unit("multi-user.target")
machine.succeed("systemd-analyze log-level debug")
print("FINDING [4-sandbox] systemd " + machine.succeed("systemctl --version | head -1").strip())
probe("nothing", [])
probe("ProtectSystem=strict", ["ProtectSystem=strict"])
probe("ProtectSystem=strict, no capabilities", ["ProtectSystem=strict", "CapabilityBoundingSet="])
probe("ProtectKernelTunables=yes", ["ProtectKernelTunables=yes"])
probe("ProtectKernelTunables=yes, no capabilities", ["ProtectKernelTunables=yes", "CapabilityBoundingSet="])
probe(
    "both, ReadWritePaths for the marker directory and the driver",
    [
        "ProtectSystem=strict",
        "ProtectKernelTunables=yes",
        "ReadWritePaths=/run/ctrlassist/hide",
        "ReadWritePaths=/sys/bus/hid/drivers/playstation",
        "CapabilityBoundingSet=",
    ],
)
probe("ProtectSystem=strict, ReadWritePaths for the marker directory", ["ProtectSystem=strict", "ReadWritePaths=/run/ctrlassist/hide"])
probe("ProtectSystem=strict, ProtectKernelTunables=yes, no ReadWritePaths", ["ProtectSystem=strict", "ProtectKernelTunables=yes"])
probe(
    "both, ReadOnlyPaths=/run /sys named, ReadWritePaths for the marker directory and the driver",
    [
        "ProtectSystem=strict",
        "ProtectKernelTunables=yes",
        "ReadOnlyPaths=/run /sys",
        "ReadWritePaths=/run/ctrlassist/hide",
        "ReadWritePaths=/sys/bus/hid/drivers/playstation",
    ],
)
probe(
    "ProtectSystem=strict, ReadOnlyPaths=/sys, ReadWritePaths for the marker directory and the driver",
    [
        "ProtectSystem=strict",
        "ReadOnlyPaths=/sys",
        "ReadWritePaths=/run/ctrlassist/hide",
        "ReadWritePaths=/sys/bus/hid/drivers/playstation",
        "CapabilityBoundingSet=",
    ],
)
probe(
    "both, ReadWritePaths for the marker directory only",
    ["ProtectSystem=strict", "ProtectKernelTunables=yes", "ReadWritePaths=/run/ctrlassist/hide", "CapabilityBoundingSet="],
)
print("FINDING [4-sandbox] journal: " + " | ".join(
    machine.succeed("journalctl -b -o cat | grep -iE 'run-p.*(is duplicate|redundant)' | sed 's/run-p[0-9]*-i[0-9]*/UNIT/' | sort | uniq -c | tail -20 || true").splitlines()
))

# Each setting of the packaged unit, added alone to a base that holds.
BASE = [
    "ProtectSystem=strict",
    "ReadOnlyPaths=/sys",
    "ReadWritePaths=/run/ctrlassist/hide",
    "ReadWritePaths=/sys/bus/hid/drivers/playstation",
    "CapabilityBoundingSet=",
]
for extra in [
    "NoNewPrivileges=yes",
    "ProtectHome=yes",
    "PrivateTmp=yes",
    "RestrictAddressFamilies=AF_NETLINK",
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
    "DevicePolicy=closed",
    "ProtectProc=invisible",
    "ProtectKernelTunables=yes",
]:
    probe(f"base + {extra}", BASE + [extra])

# The three that imply MountAPIVFS=, kept, with /run and /sys made read-only
# by entries that outrank the API file systems' own.
probe(
    "ProtectSystem=strict, the three, ReadOnlyPaths=/run, BindReadOnlyPaths=/sys",
    [
        "ProtectSystem=strict",
        "ProtectKernelTunables=yes",
        "ProtectControlGroups=yes",
        "ProtectProc=invisible",
        "ReadOnlyPaths=/run",
        "BindReadOnlyPaths=/sys",
        "ReadWritePaths=/run/ctrlassist/hide",
        "ReadWritePaths=/sys/bus/hid/drivers/playstation",
        "CapabilityBoundingSet=",
    ],
)
probe(
    "ProtectSystem=strict, the three, MountAPIVFS=no",
    [
        "ProtectSystem=strict",
        "ProtectKernelTunables=yes",
        "ProtectControlGroups=yes",
        "ProtectProc=invisible",
        "MountAPIVFS=no",
        "ReadWritePaths=/run/ctrlassist/hide",
        "ReadWritePaths=/sys/bus/hid/drivers/playstation",
        "CapabilityBoundingSet=",
    ],
)
