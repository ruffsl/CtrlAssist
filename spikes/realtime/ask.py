"""Asks rtkit to make this thread real-time, and says what happened.

Run as: ask.py <priority> [rttime-usec [spin]]. With no second argument
the process sets no RLIMIT_RTTIME, to see whether rtkit insists on one.
With `spin` it then never blocks again, to see what the limit does to a
real-time thread that runs away.
"""
import ctypes
import os
import resource
import sys

import dbus

priority = int(sys.argv[1])
rttime = int(sys.argv[2]) if len(sys.argv) > 2 else None

SCHED_RESET_ON_FORK = 0x40000000
names = {0: "other", 1: "fifo", 2: "rr"}

if rttime is not None:
    resource.setrlimit(resource.RLIMIT_RTTIME, (rttime, rttime))
# rtkit is said to want this set before it will act.
os.sched_setscheduler(0, os.SCHED_OTHER | SCHED_RESET_ON_FORK, os.sched_param(0))

bus = dbus.SystemBus()
kit = bus.get_object("org.freedesktop.RealtimeKit1", "/org/freedesktop/RealtimeKit1")
props = dbus.Interface(kit, "org.freedesktop.DBus.Properties")
limits = {
    name: int(props.Get("org.freedesktop.RealtimeKit1", name))
    for name in ("MaxRealtimePriority", "RTTimeUSecMax")
}
tid = ctypes.CDLL(None).syscall(186)  # gettid on x86_64
try:
    dbus.Interface(kit, "org.freedesktop.RealtimeKit1").MakeThreadRealtime(
        dbus.UInt64(tid), dbus.UInt32(priority)
    )
    outcome = "granted"
except dbus.DBusException as error:
    outcome = "refused: " + error.get_dbus_message()
policy = os.sched_getscheduler(0) & ~SCHED_RESET_ON_FORK
print(
    f"rtkit priority={priority} rttime={rttime} {outcome}; "
    f"policy={names.get(policy, policy)} limits={limits}"
)

if len(sys.argv) > 3:
    sys.stdout.flush()
    while True:
        pass
