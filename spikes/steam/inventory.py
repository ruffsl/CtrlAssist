# What the kernel and udev have made of every uhid device now, as one
# JSON object: the HID devices, their input devices with their codes and
# ranges, the nodes with udev's properties and the drivers above them,
# the raw nodes and the batteries; and, for each joystick node, what the
# hiding service's checks in crates/ctrlassist-hider/src/pad.rs would say
# of it, written again here from that file.
#
#   inventory.py <include/linux>          everything, as above
#   inventory.py <include/linux> names    each code's name, by type and number
import fcntl
import glob
import json
import os
import re
import struct
import subprocess
import sys

UHID = "/sys/devices/virtual/misc/uhid"

names = {}
for header in ["input-event-codes.h", "input.h"]:
    for line in open(os.path.join(sys.argv[1], header)):
        found = re.match(r"#define\s+((?:KEY|BTN|ABS|REL|MSC|FF|INPUT_PROP|EV)_\w+)\s+(0x[0-9a-fA-F]+|\d+)", line)
        if found:
            kind = found.group(1).split("_")[0]
            if kind == "BTN":
                kind = "KEY"
            if found.group(1).startswith("INPUT_PROP"):
                kind = "PROP"
            names.setdefault((kind, int(found.group(2), 0)), found.group(1))

if sys.argv[2:] == ["names"]:
    json.dump({f"{kind}:{code}": name for (kind, code), name in names.items()}, sys.stdout)
    sys.exit(0)


def read(path):
    try:
        with open(path) as file:
            return file.read().strip()
    except OSError:
        return None


def bits(bitmap):
    """Codes set in a sysfs capability bitmap: hex words of 64 bits, most significant first."""
    if bitmap is None:
        return []
    codes = []
    for index, word in enumerate(reversed(bitmap.split())):
        value = int(word, 16)
        for bit in range(64):
            if value >> bit & 1:
                codes.append(index * 64 + bit)
    return codes


def named(kind, codes):
    return [names.get((kind, code), f"{kind}_{code:#x}") for code in codes]


def udev(path):
    out = subprocess.run(["udevadm", "info", "--query=property", f"--path={path}"], capture_output=True, text=True).stdout
    return dict(line.split("=", 1) for line in out.splitlines() if "=" in line)


def chain(path):
    """udevadm info -a: the devices above, nearest first, each with its kernel name, subsystem and driver."""
    out = subprocess.run(["udevadm", "info", "-a", f"--path={path}"], capture_output=True, text=True).stdout
    found = []
    for block in out.split("\n\n"):
        kernels = re.search(r'KERNELS?=="([^"]*)"', block)
        subsystem = re.search(r'SUBSYSTEMS?=="([^"]*)"', block)
        driver = re.search(r'DRIVERS?=="([^"]*)"', block)
        if kernels:
            found.append({"kernel": kernels.group(1), "subsystem": subsystem and subsystem.group(1), "driver": driver and driver.group(1)})
    return found


def absinfo(node, code):
    request = (2 << 30) | (24 << 16) | (ord("E") << 8) | (0x40 + code)
    with open(node, "rb") as file:
        value, minimum, maximum, fuzz, flat, resolution = struct.unpack("6i", fcntl.ioctl(file, request, bytes(24)))
    return {"min": minimum, "max": maximum, "fuzz": fuzz, "flat": flat, "res": resolution}


def node_of(device):
    devname = read(f"{device}/uevent") or ""
    found = re.search(r"^DEVNAME=(.*)$", devname, re.M)
    return f"/dev/{found.group(1)}" if found else None


def mode(node):
    try:
        info = os.stat(node)
    except OSError:
        return None
    return {"mode": oct(info.st_mode & 0o7777), "uid": info.st_uid, "gid": info.st_gid}


def input_device(path):
    caps = {kind: read(f"{path}/capabilities/{kind}") for kind in ["ev", "key", "abs", "rel", "msc", "ff", "sw"]}
    nodes = []
    for child in sorted(glob.glob(f"{path}/event*") + glob.glob(f"{path}/js*") + glob.glob(f"{path}/mouse*")):
        props = udev(child)
        nodes.append({"sys": child, "node": node_of(child), "access": mode(node_of(child)), "udev": props, "chain": chain(child)})
    abs_codes = bits(caps["abs"])
    event = next((node["node"] for node in nodes if node["node"] and "event" in node["node"]), None)
    ranges = {}
    if event:
        for code in abs_codes:
            ranges[names.get(("ABS", code), str(code))] = absinfo(event, code)
    return {
        "sys": path,
        "name": read(f"{path}/name"),
        "phys": read(f"{path}/phys"),
        "uniq": read(f"{path}/uniq"),
        "id": {part: read(f"{path}/id/{part}") for part in ["bustype", "vendor", "product", "version"]},
        "properties": named("PROP", bits(read(f"{path}/properties"))),
        "capabilities": caps,
        "ev": named("EV", bits(caps["ev"])),
        "key": named("KEY", bits(caps["key"])),
        "abs": ranges,
        "rel": named("REL", bits(caps["rel"])),
        "msc": named("MSC", bits(caps["msc"])),
        "ff": named("FF", bits(caps["ff"])),
        "udev": udev(path),
        "nodes": nodes,
    }


def keys_all_high(bitmap):
    words = bitmap.split(" ")
    shaped = all(1 <= len(word) <= 16 and re.fullmatch(r"[0-9a-f]+", word) for word in words)
    low = 2 if any(len(word) > 8 for word in words) else 4
    return bool(shaped and len(words) > low and all(word == "0" for word in words[-low:]))


def flag(props, name):
    return props.get(name) == "1"


def is_only_a_pad(props, keys):
    return (
        flag(props, "ID_INPUT_JOYSTICK")
        and not any(flag(props, name) for name in ["ID_INPUT_KEYBOARD", "ID_INPUT_MOUSE", "ID_INPUT_TOUCHPAD", "ID_INPUT_TABLET", "ID_INPUT_POINTINGSTICK"])
        and (not flag(props, "ID_INPUT_KEY") or (keys is not None and keys_all_high(keys)))
    )


def hider(hids, inputs):
    """For each event node, what pad.rs's checks would say."""
    verdicts = []
    for device in inputs:
        for node in device["nodes"]:
            if not node["node"] or "event" not in node["node"]:
                continue
            props = node["udev"]
            only = is_only_a_pad(props, device["capabilities"]["key"])
            parent = next((hid for hid in hids if device["sys"].startswith(hid["sys"] + "/")), None)
            under = [other for other in inputs if parent and other["sys"].startswith(parent["sys"] + "/")]
            sharers = [
                other["name"]
                for other in under
                for candidate in [other["udev"]] + [n["udev"] for n in other["nodes"]]
                if any(flag(candidate, name) for name in ["ID_INPUT_KEYBOARD", "ID_INPUT_MOUSE", "ID_INPUT_TABLET", "ID_INPUT_POINTINGSTICK"])
            ]
            uniq = device["uniq"] or ""
            address = re.fullmatch(r"([0-9a-f]{2}:){5}[0-9a-f]{2}", uniq) is not None
            serial = props.get("ID_SERIAL_SHORT")
            markers = []
            if parent and re.fullmatch(r"[0-9A-F]{4}:[0-9A-F]{4}:[0-9A-F]{4}\.[0-9A-F]{4,8}", parent["kernel"]):
                markers.append(f"hid-{parent['kernel']}")
                hid_uniq, hid_id = parent["udev"].get("HID_UNIQ"), parent["udev"].get("HID_ID")
                if hid_uniq and re.fullmatch(r"[0-9A-Za-z:]{1,64}", hid_uniq) and hid_id:
                    markers.append(f"uniq-{hid_id}-{hid_uniq}")
            if address:
                markers.append(f"addr-{uniq}")
            elif serial:
                markers.append(f"serial-{device['id']['vendor']}-{device['id']['product']}-{serial}")
            verdicts.append({
                "node": node["node"],
                "input": device["name"],
                "is_only_a_pad": only,
                "seat_tagged": ":uaccess:" in props.get("CURRENT_TAGS", ""),
                "parent": parent and parent["kernel"],
                "parent_driver": parent and parent["driver"],
                "shares_with_keyboard_or_pointer": sorted(set(sharers)),
                "identify": "pad" if only and parent and not sharers else ("NotAPad" if not only else "Unsupported"),
                "markers": sorted(markers),
                "raw_node": "offered" if parent and parent["driver"] == "playstation" else "Unsupported (driver not in RAW_DRIVERS)",
                "raw_children_of_parent": parent and parent["hidraw"],
                "uniq_is_address": address,
            })
    return verdicts


hids = []
for path in sorted(glob.glob(f"{UHID}/*:*")):
    kernel = os.path.basename(path)
    driver = os.path.basename(os.readlink(f"{path}/driver")) if os.path.exists(f"{path}/driver") else None
    uevent = read(f"{path}/uevent")
    hids.append({
        "sys": path,
        "kernel": kernel,
        "driver": driver,
        "uevent": dict(line.split("=", 1) for line in uevent.splitlines()) if uevent else {},
        "udev": udev(path),
        "uevent_file": mode(f"{path}/uevent"),
        "hidraw": [os.path.basename(raw) for raw in sorted(glob.glob(f"{path}/hidraw/hidraw*"))],
        "inputs": [os.path.basename(i) for i in sorted(glob.glob(f"{path}/input/input*"))],
        "power_supply": [os.path.basename(p) for p in sorted(glob.glob(f"{path}/power_supply/*"))],
    })

inputs = [input_device(os.path.realpath(path)) for path in sorted(glob.glob("/sys/class/input/input*")) if os.path.realpath(path).startswith(UHID)]

raws = []
for path in sorted(glob.glob("/sys/class/hidraw/hidraw*")):
    real = os.path.realpath(path)
    if not real.startswith(UHID):
        continue
    raws.append({"sys": real, "node": node_of(real), "access": mode(node_of(real)), "udev": udev(real), "chain": chain(real)})

supplies = []
for path in sorted(glob.glob("/sys/class/power_supply/*")):
    real = os.path.realpath(path)
    if not real.startswith(UHID):
        continue
    uevent = read(f"{real}/uevent") or ""
    supplies.append({"sys": real, "uevent": dict(line.split("=", 1) for line in uevent.splitlines() if "=" in line), "udev": udev(real)})

json.dump({"hid": hids, "input": inputs, "hidraw": raws, "power_supply": supplies, "hider": hider(hids, inputs)}, sys.stdout)
