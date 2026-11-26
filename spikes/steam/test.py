# The spike's script: three stand-ins one after another (the descriptor
# written here, the same with report 0x47 declared, and the real
# controller's with its mouse and keyboard), each bound, looked at, played
# with in lizard mode and out of it, and opened through its raw node.
# Everything seen is printed as SPIKE lines and kept in finding.json; the
# checks are asserted only at the end, so that one that fails loses no
# observation.
import json
import os
import time

LOGS = "/run/spike"
finding = {}
checks = []


def say(name, value):
    finding[name] = value
    print("SPIKE " + json.dumps({name: value}, sort_keys=True))


def check(name, passed, detail=None):
    checks.append((name, bool(passed), detail))
    print(f"CHECK {'pass' if passed else 'FAIL'}: {name}" + (f" ({detail})" if detail is not None else ""))


def lines(path):
    text = machine.succeed(f"cat {path} 2>/dev/null || true")
    return [json.loads(line) for line in text.splitlines() if line.strip()]


def now():
    return float(machine.succeed("steam-spike now"))


def command(fifo, line):
    machine.succeed(f"echo '{line}' > {fifo}")


def inventory():
    return json.loads(machine.succeed("spike-inventory"))


NAMES = {}


def code_name(kind, code):
    prefix = {0: "EV", 1: "KEY", 2: "REL", 3: "ABS", 4: "MSC"}.get(kind)
    return NAMES.get(f"{prefix}:{code}", f"{kind}:{code}") if prefix else f"{kind}:{code}"


def uevents(t0, t1):
    """The kernel's uevents between two times, each as (ms after t0, action, subsystem, devpath, name)."""
    found = []
    for event in lines(f"{LOGS}/uevents.jsonl"):
        if "header" not in event or not t0 <= event["t"] <= t1:
            continue
        props = event["properties"]
        found.append({
            "ms": round((event["t"] - t0) * 1000, 1),
            "action": props.get("ACTION"),
            "subsystem": props.get("SUBSYSTEM"),
            "devpath": props.get("DEVPATH"),
            "name": props.get("NAME") or props.get("DEVNAME") or props.get("POWER_SUPPLY_NAME") or props.get("HID_NAME"),
        })
    return found


def fake_heard(variant, t0, t1):
    """What the kernel asked of or sent the stand-in between two times."""
    return [
        {k: v for k, v in event.items() if k != "t"} | {"ms": round((event["t"] - t0) * 1000, 1)}
        for event in lines(f"{LOGS}/{variant}/fake.jsonl")
        if t0 <= event["t"] <= t1 and event["ev"] in ("start", "stop", "open", "close", "output", "get_report", "set_report", "uhid")
    ]


def events(path, t0, t1):
    """Input events read between two times, by name, without the report ends (SYN_REPORT)."""
    return [
        (code_name(event["type"], event["code"]), event["value"])
        for event in lines(path)
        if "type" in event and t0 <= event["t"] <= t1 and event["type"] != 0
    ]


def ended(path):
    return [event for event in lines(path) if event.get("ev") in ("read error", "end")]


def node_named(name):
    return machine.succeed(
        "for i in /sys/class/input/input*; do "
        f"if [ \"$(cat $i/name)\" = '{name}' ]; then for e in $i/event*; do echo /dev/input/$(basename $e); done; fi; done"
    ).strip()


def window(action, settle):
    t0 = now()
    action()
    time.sleep(settle)
    return t0, now()


machine.wait_for_unit("multi-user.target")
NAMES = json.loads(machine.succeed("spike-inventory names"))
machine.succeed(f"mkdir -p {LOGS}")
say("kernel", machine.succeed("uname -r").strip())
say("config", machine.succeed("zcat /proc/config.gz | grep -E '^(# )?CONFIG_(HID_STEAM|STEAM_FF|UHID|HIDRAW|HID_GENERIC)[ =]'").strip().splitlines())
say("lizard_mode_default", machine.succeed("cat /sys/module/hid_steam/parameters/lizard_mode").strip())
say("hid_drivers", machine.succeed("ls /sys/bus/hid/drivers/").split())
hider_gid = int(machine.succeed("getent group ctrlassist-hider | cut -d: -f3"))
say("hider_gid", hider_gid)

machine.succeed(f"systemd-run --unit uevents steam-spike uevents {LOGS}/uevents.jsonl")
machine.wait_until_succeeds(f"grep -q listening {LOGS}/uevents.jsonl")

# The report as the stand-in builds it: 32 bits of buttons, little-endian,
# from byte 2. A is byte 2 bit 0; menu (BTN_START) byte 2 bit 6; the left
# trackpad's touch byte 5 bit 1 and the right's byte 4 bit 5.
A, MENU, LEFT_PAD_TOUCH, RIGHT_PAD_TOUCH = 0x1, 0x40, 1 << 25, 1 << 21


def bind(variant):
    """Starts the stand-in and waits for the gamepad and motion devices; says what came and how fast."""
    d = f"{LOGS}/{variant}"
    machine.succeed(f"mkdir -p {d} && mkfifo {d}/fake.fifo {d}/raw.fifo")
    mark = int(machine.succeed("dmesg | wc -l"))
    machine.succeed(f"systemd-run --unit fake-{variant} steam-spike fake {variant} {d}/fake.jsonl {d}/fake.fifo")
    machine.wait_until_succeeds("grep -qx 'Steam Controller' /sys/class/input/input*/name", timeout=60)
    machine.wait_until_succeeds("grep -qx 'Steam Controller Motion Sensors' /sys/class/input/input*/name", timeout=60)
    machine.succeed("udevadm settle")
    time.sleep(0.5)
    created = next(event["t"] for event in lines(f"{d}/fake.jsonl") if event["ev"] == "create")
    t1 = now()
    timeline = [event for event in uevents(created, t1) if event["action"] == "add"]
    say(f"{variant}.bind.uevents", timeline)
    gamepad = [event for event in timeline if event["subsystem"] == "input" and event["name"] == '"Steam Controller"']
    say(f"{variant}.bind.ms_to_gamepad_input", gamepad[0]["ms"] if gamepad else None)
    say(f"{variant}.bind.fake_heard", fake_heard(variant, created, t1))
    say(f"{variant}.bind.dmesg", machine.succeed(f"dmesg | tail -n +{mark + 1}").splitlines())
    found = inventory()
    say(f"{variant}.bind.inventory", found)
    check(f"{variant}: hid-steam binds the stand-in and makes the gamepad", gamepad)
    return d, found


def open_raw(variant, d):
    """Opens the raw node from another process; says what went and how fast."""
    raws = inventory()["hidraw"]
    say(f"{variant}.raw.nodes", [{"node": raw["node"], "hid": raw["sys"].split("/")[-3], "access": raw["access"]} for raw in raws])
    node = raws[0]["node"]
    machine.succeed(f"systemd-run --unit raw-{variant} steam-spike raw {node} {d}/raw.jsonl {d}/raw.fifo")
    machine.wait_until_succeeds(f"grep -q opened {d}/raw.jsonl")
    machine.wait_until_fails("grep -qx 'Steam Controller' /sys/class/input/input*/name", timeout=30)
    machine.succeed("udevadm settle")
    time.sleep(0.5)
    opened = next(event for event in lines(f"{d}/raw.jsonl") if event["ev"] == "opened")
    say(f"{variant}.raw.opened", {k: v for k, v in opened.items() if k != "t"})
    t1 = now()
    say(f"{variant}.raw.open_uevents", uevents(opened["t"], t1))
    say(f"{variant}.raw.open_fake_heard", fake_heard(variant, opened["t"], t1))
    found = inventory()
    say(f"{variant}.raw.open_inventory_inputs", [(i["name"], [n["node"] for n in i["nodes"]]) for i in found["input"]])
    say(f"{variant}.raw.open_power_supply", [p["sys"] for p in found["power_supply"]])
    return node


def through_raw(variant, d, label, fake_line=None, raw_line=None, settle=0.5):
    """Something sent one way or the other while the raw node is open; what each end saw."""
    t0 = now()
    if fake_line:
        command(f"{d}/fake.fifo", fake_line)
    if raw_line:
        command(f"{d}/raw.fifo", raw_line)
    time.sleep(settle)
    t1 = now()
    sent = [event["data"] for event in lines(f"{d}/fake.jsonl") if event["ev"] == "sent" and t0 <= event["t"] <= t1]
    read = [event["data"] for event in lines(f"{d}/raw.jsonl") if event["ev"] == "read" and t0 <= event["t"] <= t1]
    replies = [{k: v for k, v in event.items() if k != "t"} for event in lines(f"{d}/raw.jsonl") if event["ev"] in ("wrote", "setfeature", "getfeature") and t0 <= event["t"] <= t1]
    heard = fake_heard(variant, t0, t1)
    say(f"{variant}.raw.{label}", {"fake_sent": sent, "raw_read": read, "raw_result": replies, "fake_heard": heard})
    return sent, read, replies, heard


def close_raw(variant, d, before):
    command(f"{d}/raw.fifo", "quit")
    machine.wait_until_succeeds("grep -qx 'Steam Controller' /sys/class/input/input*/name", timeout=60)
    machine.wait_until_succeeds("grep -qx 'Steam Controller Motion Sensors' /sys/class/input/input*/name", timeout=60)
    machine.succeed("udevadm settle")
    time.sleep(0.5)
    closed = next(event["t"] for event in lines(f"{d}/raw.jsonl") if event["ev"] == "closing")
    t1 = now()
    say(f"{variant}.raw.close_uevents", uevents(closed, t1))
    heard = fake_heard(variant, closed, t1)
    say(f"{variant}.raw.close_fake_heard", heard)
    found = inventory()
    after = [(i["name"], [n["node"] for n in i["nodes"]]) for i in found["input"]]
    say(f"{variant}.raw.close_inventory_inputs", {"before": before, "after": after})
    say(f"{variant}.raw.close_power_supply", [p["sys"] for p in found["power_supply"]])
    check(f"{variant}: the devices come back when the raw node is closed", any(name == "Steam Controller" for name, _ in after))
    return heard


def stop(variant, d):
    t0 = now()
    command(f"{d}/fake.fifo", "quit")
    machine.wait_until_fails("grep -qx 'Steam Controller' /sys/class/input/input*/name", timeout=30)
    machine.succeed("udevadm settle")
    say(f"{variant}.stop.uevents", [e for e in uevents(t0, now()) if e["action"] == "remove"])


def requests(heard):
    """The feature commands and output reports in what the stand-in heard, briefly: (kind, first bytes)."""
    out = []
    for event in heard:
        if event["ev"] == "set_report":
            out.append(("SET", event["data"][:20]))
        elif event["ev"] == "get_report":
            out.append(("GET", (event["reply"] or "error")[:8]))
        elif event["ev"] == "output":
            out.append(("OUT", event["data"]))
    return out


def hex_report(*parts, size):
    data = bytes(parts)
    return (data + bytes(size - len(data))).hex()


# --- The descriptor written here ------------------------------------------

d, found = bind("own")
inputs = {i["name"]: i for i in found["input"]}
pad, motion = node_named("Steam Controller"), node_named("Steam Controller Motion Sensors")
say("own.nodes", {"pad": pad, "motion": motion})
check("own: the driver asks the serial and the attributes, and answers come back", any(e["ev"] == "get_report" and e["reply"] and e["reply"].startswith("01ae") for e in finding["own.bind.fake_heard"]))
check("own: no force feedback on the gamepad", inputs["Steam Controller"]["ff"] == [], inputs["Steam Controller"]["ff"])

machine.succeed(f"systemd-run --unit ev-own-pad steam-spike events {pad} {d}/pad-events.jsonl")
machine.succeed(f"systemd-run --unit ev-own-motion steam-spike events {motion} {d}/motion-events.jsonl")
machine.wait_until_succeeds(f"grep -q opened {d}/pad-events.jsonl && grep -q opened {d}/motion-events.jsonl")
fake_fifo = f"{d}/fake.fifo"


def play(label, buttons=A | LEFT_PAD_TOUCH, count=10):
    """A pressed and the left stick and trackpad moved, then let go; what each node said."""
    t0, t1 = window(lambda: command(fake_fifo, f"state 42 {buttons:08x} 20000 -20000 {count} 10"), 0.6)
    t1b = window(lambda: command(fake_fifo, "state 42 00000000 0 0 3 10"), 0.4)[1]
    pad_events = events(f"{d}/pad-events.jsonl", t0, t1b)
    motion_events = events(f"{d}/motion-events.jsonl", t0, t1b)
    heard = fake_heard("own", t0, t1b)
    say(f"own.lizard.{label}", {"pad_events": pad_events[:60], "pad_event_count": len(pad_events), "motion_event_count": len(motion_events), "motion_events": motion_events[:14], "fake_heard": requests(heard)})
    return pad_events, motion_events


def hold_menu(label, held_ms=700):
    """The menu button held, then let go; what the stand-in was sent and said meanwhile."""
    t0 = now()
    command(fake_fifo, f"state 42 {MENU:08x} 0 0 {held_ms // 10} 10")
    time.sleep(held_ms / 1000 + 0.4)
    command(fake_fifo, "state 42 00000000 0 0 3 10")
    time.sleep(0.5)
    t1 = now()
    first = next(e["t"] for e in lines(f"{d}/fake.jsonl") if e["ev"] == "sent" and e["t"] >= t0 and e["data"][4:6] == "40")
    heard = fake_heard("own", first, t1)
    pad_events = events(f"{d}/pad-events.jsonl", first, t1)
    say(f"own.lizard.{label}", {"after_first_menu_report": heard, "pad_events": pad_events, "motion_event_count": len(events(f"{d}/motion-events.jsonl", first, t1))})
    return heard, pad_events


silent_pad, silent_motion = play("default_A_and_stick")
check("own: at lizard_mode's default the gamepad and the motion device say nothing", not silent_pad and not silent_motion, (len(silent_pad), len(silent_motion)))

heard, menu_events = hold_menu("hold_menu_to_gamepad")
check("own: holding menu sends the stand-in commands", any(e["ev"] == "set_report" for e in heard), requests(heard))
first_command = next((e["ms"] for e in heard if e["ev"] in ("set_report", "output")), None)
say("own.lizard.ms_from_menu_to_first_command", first_command)

flowing_pad, flowing_motion = play("gamepad_mode_A_and_stick")
check("own: after the hold, the gamepad says what it is sent", flowing_pad, len(flowing_pad))

heard_back, _ = hold_menu("hold_menu_again")
silent_again, _ = play("back_in_lizard_A_and_stick")

t0, t1 = window(lambda: machine.succeed("echo 0 > /sys/module/hid_steam/parameters/lizard_mode"), 0.5)
say("own.lizard.param_off_fake_heard", requests(fake_heard("own", t0, t1)))
param_pad, param_motion = play("param_off_A_and_stick")
check("own: with lizard_mode=0 events flow at once", param_pad, len(param_pad))
check("own: with lizard_mode=0 motion flows", param_motion, len(param_motion))
# A trackpad touched and untouched, with lizard mode off: is the
# position reported only while touched?
untouched, _ = play("param_off_pads_untouched", buttons=A)
both, _ = play("param_off_both_pads_touched", buttons=A | LEFT_PAD_TOUCH | RIGHT_PAD_TOUCH)
t0, t1 = window(lambda: machine.succeed("echo 1 > /sys/module/hid_steam/parameters/lizard_mode"), 0.5)
say("own.lizard.param_on_fake_heard", requests(fake_heard("own", t0, t1)))

# Battery, at lizard_mode's default: does the power supply follow it?
t0, t1 = window(lambda: command(fake_fifo, "send " + hex_report(0x43, 1, 77, 0x10, 0x0e, size=15)), 0.4)
battery = found["power_supply"][0]["sys"] if found["power_supply"] else None
if battery:
    say("own.battery.after_report", machine.succeed(f"cat {battery}/uevent").splitlines())

before = [(i["name"], [n["node"] for n in i["nodes"]]) for i in inventory()["input"]]
open_raw("own", d)
say("own.raw.event_readers_ended", {"pad": ended(f"{d}/pad-events.jsonl"), "motion": ended(f"{d}/motion-events.jsonl")})
check("own: opening the raw node removes the gamepad and motion devices", not [i for i in finding["own.raw.open_inventory_inputs"] if i[0].startswith("Steam Controller")])
check("own: the driver sends the stand-in nothing as the raw node opens", not [e for e in finding["own.raw.open_fake_heard"] if e["ev"] in ("set_report", "get_report", "output")])

sent, read, _, _ = through_raw("own", d, "report_42", fake_line=f"state 42 {A:08x} 20000 -20000 1 10")
check("own: a 0x42 report is read from the raw node byte for byte", sent and read == sent, (sent, read))
sent, read, _, _ = through_raw("own", d, "report_47_undeclared", fake_line="send " + hex_report(0x47, 1, 1, size=46))
say("own.raw.report_47_undeclared_read", bool(read))
sent, read, _, _ = through_raw("own", d, "report_45", fake_line=f"state 45 {A:08x} 20000 -20000 1 10")
check("own: a 0x45 report is read from the raw node byte for byte", sent and read == sent, (sent, read))
through_raw("own", d, "report_43", fake_line="send " + hex_report(0x43, 1, 77, size=15))
through_raw("own", d, "report_79", fake_line="send 7902")
through_raw("own", d, "report_42_short", fake_line="send " + hex_report(0x42, 1, 1, size=30))
_, _, result, heard = through_raw("own", d, "write_80", raw_line="write " + hex_report(0x80, 0, 0x00, 0x00, 0xff, 0x7f, 0x02, 0xff, 0x7f, 0x00, size=10))
check("own: an output report 0x80 written to the raw node reaches the stand-in", any(e["ev"] == "output" and e["data"].startswith("80") for e in heard), heard)
_, _, result, heard = through_raw("own", d, "setfeature_87", raw_line="setfeature " + hex_report(0x01, 0x87, 0x03, 0x09, 0x00, 0x00, size=64))
check("own: a feature SET written to the raw node reaches the stand-in", any(e["ev"] == "set_report" and e["data"].startswith("01870309") for e in heard), heard)
through_raw("own", d, "getfeature_after_87", raw_line="getfeature " + hex_report(0x01, size=64))
# The serial asked as Steam and SDL ask it: the command, then a GET.
through_raw("own", d, "setfeature_ae", raw_line="setfeature " + hex_report(0x01, 0xae, 0x15, 0x01, size=64))
through_raw("own", d, "getfeature_after_ae", raw_line="getfeature " + hex_report(0x01, size=64))
# Menu held while the raw node is open: does the driver switch modes?
t0, t1 = window(lambda: command(fake_fifo, f"state 42 {MENU:08x} 0 0 70 10"), 1.1)
say("own.raw.menu_held_fake_heard", requests(fake_heard("own", t0, t1)))
command(fake_fifo, "state 42 00000000 0 0 3 10")

heard = close_raw("own", d, before)
say("own.raw.close_requests", requests(heard))
check("own: the driver asks again for the serial and attributes as the raw node closes", any(e["ev"] == "get_report" for e in heard), requests(heard))
stop("own", d)

# --- The same with 0x47 declared ------------------------------------------

d, found = bind("own47")
fake_fifo = f"{d}/fake.fifo"
pad = node_named("Steam Controller")
machine.succeed("echo 0 > /sys/module/hid_steam/parameters/lizard_mode")
machine.succeed(f"systemd-run --unit ev-own47-pad steam-spike events {pad} {d}/pad-events.jsonl")
machine.wait_until_succeeds(f"grep -q opened {d}/pad-events.jsonl")
t0, t1 = window(lambda: command(fake_fifo, f"state 47 {A:08x} 20000 -20000 5 10"), 0.6)
say("own47.lizard_off.report_47_pad_events", events(f"{d}/pad-events.jsonl", t0, t1))
machine.succeed("echo 1 > /sys/module/hid_steam/parameters/lizard_mode")
before = [(i["name"], [n["node"] for n in i["nodes"]]) for i in inventory()["input"]]
open_raw("own47", d)
sent, read, _, _ = through_raw("own47", d, "report_47_declared", fake_line="send " + hex_report(0x47, 1, 1, size=46))
check("own47: a declared 0x47 report is read from the raw node byte for byte", sent and read == sent, (sent, read))
through_raw("own47", d, "report_47_short", fake_line="send " + hex_report(0x47, 1, 1, size=40))
close_raw("own47", d, before)
stop("own47", d)

# --- The real controller's descriptor, with its mouse and keyboard --------

d, found = bind("real")
fake_fifo = f"{d}/fake.fifo"
others = [i for i in found["input"] if not i["name"].startswith("Steam Controller")]
say("real.other_inputs", [{"name": i["name"], "sys": i["sys"], "nodes": [n["node"] for n in i["nodes"]], "key_count": len(i["key"]), "rel": i["rel"]} for i in others])
check("real: the mouse and keyboard collections make input devices of their own", others)
check("real: the hiding service would refuse the gamepad, its HID device sharing a keyboard or pointer",
      all(v["identify"] == "Unsupported" for v in found["hider"] if v["input"] == "Steam Controller"), found["hider"])
other_nodes = [n["node"] for i in others for n in i["nodes"] if n["node"] and "event" in n["node"]]
for index, node in enumerate(other_nodes):
    machine.succeed(f"systemd-run --unit ev-real-{index} steam-spike events {node} {d}/other-{index}-events.jsonl")
    machine.wait_until_succeeds(f"grep -q opened {d}/other-{index}-events.jsonl")


def lizard_reports(label):
    """A key (Enter) pressed and let go, and the pointer moved, as the controller's own lizard mode sends them."""
    t0 = now()
    for line in ["send " + hex_report(0x41, 0, 0, 0x28, size=9), "send " + hex_report(0x41, size=9), "send " + hex_report(0x40, 0, 5, 0xfb, size=6)]:
        command(fake_fifo, line)
        time.sleep(0.05)
    time.sleep(0.4)
    t1 = now()
    seen = {node: events(f"{d}/other-{index}-events.jsonl", t0, t1) for index, node in enumerate(other_nodes)}
    say(f"real.{label}", seen)
    return seen


seen = lizard_reports("lizard_keyboard_and_mouse")
check("real: the keyboard and mouse reports come out of their input devices", any(seen.values()), seen)
before = [(i["name"], [n["node"] for n in i["nodes"]]) for i in inventory()["input"]]
open_raw("real", d)
check("real: the keyboard and mouse stay while the raw node is open", [i for i in finding["real.raw.open_inventory_inputs"] if not i[0].startswith("Steam Controller")])
lizard_reports("raw_open_keyboard_and_mouse")
through_raw("real", d, "report_47_undeclared", fake_line="send " + hex_report(0x47, 1, 1, size=46))
through_raw("real", d, "report_44", fake_line="send " + hex_report(0x44, 1, 2, 3, 4, 5, size=6))
sent, read, _, _ = through_raw("real", d, "report_42", fake_line=f"state 42 {A:08x} 20000 -20000 1 10")
check("real: a 0x42 report is read from the raw node byte for byte", sent and read == sent, (sent, read))
close_raw("real", d, before)
stop("real", d)

machine.succeed("systemctl stop uevents")
machine.succeed(f"find {LOGS} -type p -delete")
machine.copy_from_vm(LOGS, "")
with open(os.path.join(machine.out_dir, "finding.json"), "w") as file:
    json.dump(finding, file, indent=1, sort_keys=True)

failed = [(name, detail) for name, passed, detail in checks if not passed]
print("CHECKS " + json.dumps([(name, passed) for name, passed, _ in checks]))
assert not failed, failed
