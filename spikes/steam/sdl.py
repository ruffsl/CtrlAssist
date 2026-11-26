# The identity's spike: what SDL3 makes of a stand-in Steam Controller,
# under the descriptor written here and under the retail one. For each,
# the stand-in is bound by hid-steam, SDL's probe is run beside it while
# the stand-in reports a button, a paddle and a trackpad touch, and what
# SDL said and what it asked of the stand-in are printed as SPIKE lines.
import json
import os
import time

LOGS = "/run/spike"
A, R4, LEFT_PAD_TOUCH = 0x1, 0x80, 0x02000000


def lines(path):
    return [json.loads(line) for line in machine.succeed(f"cat {path}").splitlines() if line.strip()]


def now():
    return float(machine.succeed("steam-spike now"))


def command(fifo, line):
    machine.succeed(f"echo '{line}' > {fifo}")


finding = {}
for variant in ["own", "real"]:
    d = f"{LOGS}/{variant}"
    machine.succeed(f"mkdir -p {d} && mkfifo {d}/fake.fifo")
    machine.succeed(f"systemd-run --unit fake-{variant} steam-spike fake {variant} {d}/fake.jsonl {d}/fake.fifo")
    machine.wait_until_succeeds("grep -qx 'Steam Controller' /sys/class/input/input*/name", timeout=60)
    machine.succeed("udevadm settle")
    time.sleep(0.5)
    t0 = now()
    machine.succeed(f"systemd-run --unit probe-{variant} -E SDL_VIDEODRIVER=dummy -p StandardOutput=file:{d}/probe.out -p StandardError=file:{d}/probe.err sdl-probe 5000")
    time.sleep(1.5)
    command(f"{d}/fake.fifo", f"state 42 {A | LEFT_PAD_TOUCH:08x} 20000 -20000 20 10")
    time.sleep(0.5)
    command(f"{d}/fake.fifo", f"state 42 {R4:08x} 0 0 20 10")
    time.sleep(0.5)
    command(f"{d}/fake.fifo", "state 42 00000000 0 0 3 10")
    machine.wait_until_succeeds(f"grep -q '^SDL done' {d}/probe.out", timeout=30)
    t1 = now()
    said = machine.succeed(f"cat {d}/probe.out").splitlines()
    heard = [
        {k: v for k, v in event.items() if k != "t"} | {"ms": round((event["t"] - t0) * 1000)}
        for event in lines(f"{d}/fake.jsonl")
        if t0 <= event["t"] <= t1 and event["ev"] in ("open", "close", "output", "get_report", "set_report")
    ]
    finding[variant] = {"sdl": said, "stand_in_heard": heard, "stderr": machine.succeed(f"cat {d}/probe.err").splitlines()}
    for line in said:
        print(f"SPIKE {variant} {line}")
    for event in heard:
        print(f"SPIKE {variant} HEARD {json.dumps(event)}")
    command(f"{d}/fake.fifo", "quit")
    machine.wait_until_fails("grep -qx 'Steam Controller' /sys/class/input/input*/name", timeout=30)
    machine.succeed("udevadm settle")
    time.sleep(0.5)

machine.succeed(f"find {LOGS} -type p -delete")
with open(os.path.join(machine.out_dir, "sdl.json"), "w") as file:
    json.dump(finding, file, indent=1, sort_keys=True)
