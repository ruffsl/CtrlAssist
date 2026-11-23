# How an unprivileged hiding service has a pad's driver re-bound

Hiding a pad means re-binding its driver: the kernel removes the pad's nodes, which cuts off every program that had them open, and makes them again under the rule that hides them. Writing a driver's `bind` and `unbind` files needs only the files' permissions (`bind_store` and `unbind_store` in `drivers/base/bus.c` call no `capable()`), but they are root's, `--w-------`. Two ways of letting a service with no capabilities have it done were proven in NixOS VMs on Linux 6.18 and udev 262 (the spike `spikes/hider-rootless/`, in the history at `934c482`, which the plan [`hider-rootless.md`](../plans/hider-rootless.md) replaced). One was chosen; this keeps the other, and the design set aside before both, so they can be revisited.

## Driver files given to the group

A shipped rule on driver objects gives the group a listed set of drivers' `bind` and `unbind` files:

```
SUBSYSTEM=="drivers", ACTION!="remove", KERNEL=="playstation|xpad|usbhid", \
  RUN+="/usr/bin/chgrp ctrlassist-hider %S%p/bind %S%p/unbind", \
  RUN+="/usr/bin/chmod g+w %S%p/bind %S%p/unbind"
```

A driver object has an add event when its module registers it, so a module loaded after boot is covered too; sysfs keeps the files' owner and mode until the driver is unregistered. Tried on the HID bus (`playstation`, with a uhid DualSense) and the USB bus (`usbhid`, with QEMU's tablet; `xpad`, with an Xbox 360 pad made as a USB gadget on `dummy_hcd`): the service's user, with no capabilities, unbound and bound each one and cut off a reader on the event and hidraw nodes; a user outside the group was refused.

It is the simpler of the two and needs nothing of udev at hiding time but the marker rule. But its reach is per driver, not per pad: a member of the group can unbind any device a listed driver has, and `usbhid` and `hid-generic` drive keyboards and mice. That is denial of service, not escalation, but new. And the list becomes a packaging decision: today's service re-binds whatever driver a pad has, so every driver a supported pad can have (`playstation`, `xpad`, `hid-generic`, `usbhid`, `hid-nintendo`, `hid-microsoft`, …) must be named, and a pad on any other falls back to a grab.

## A request to udev, a pad at a time (chosen)

The service writes a synthetic event with an argument of its own to the `uevent` file of the pad's HID device or USB interface, and a rule, run by udev as root, does the unbind and the probe:

```
SUBSYSTEM=="hid|usb", ACTION=="change", ENV{SYNTH_ARG_CTRLASSIST}=="rebind", \
  ATTR{driver/unbind}="%k", ATTR{subsystem/drivers_probe}="%k"
```

The `uevent` file is given to the group by the rule only for devices already known to be pads: the event node of a joystick that is no keyboard or mouse has its HID device's (or USB interface's) file `chgrp`ed and made group-writable. Tried in the spike on the uhid DualSense: the reader was cut off and udev settled in 0.09 s, with no deadlock, since the unbind only queues the children's remove events behind the parent's change. Synthetic uevent arguments need Linux 4.13 or later.

**Chosen** because it reaches pads only and needs no list: any driver a pad has can be re-bound, and nothing that isn't a pad can. Its cost: the group can send any synthetic event (add, change, remove) to any pad's HID device or USB interface, which udev processes as root through the stock rules; and the rule has to find the pad's parent from the event node.

## What would bring the other back

- **The request failing for some kind of pad:** a driver whose probe waits on udev, or a pad whose event node's parent the rule can't reach. The spike tried it on a uhid pad alone; the plan tries a USB gadget pad under `xpad` in the VM and real pads on USB and Bluetooth.
- **A middle path:** the request for every pad, plus the driver files of the PlayStation and Xbox drivers only, for the pads most used, should the request prove fragile for them in particular. Neither driver drives keyboards.

## Set aside: a root kept and shrunk

Keeping root but with the static marker rule (no rules written at runtime, no reload), `CapabilityBoundingSet=` empty, `ProtectKernelTunables=yes` with only the marker directory and the driver directories writable, `DevicePolicy=closed` and `ProtectProc=invisible`. Hiding and restoring worked with every one of those settings. It was set aside because it isn't confined as written:

- **`/run` and `/sys` stayed writable,** so the process could still write a rule into `/run/udev/rules.d`, the very thing the redesign is about. On systemd 262, adding any one of `ProtectKernelTunables=yes`, `ProtectControlGroups=yes` or `ProtectProc=invisible` to `ProtectSystem=strict` makes `/run` writable again and leaves `/sys` writable: those settings imply `MountAPIVFS=`, whose fallback entries for `/run` and `/sys` outrank the read-only ones (`src/core/namespace.c`). `ReadOnlyPaths=/run` and `BindReadOnlyPaths=/sys` put it right.
- **Root with no capabilities is still root to systemd:** PID 1's private socket accepts any uid-0 peer, and `StartUnit` over the system bus queued a job and ran it. Only `RestrictAddressFamilies=AF_NETLINK`, with no Unix sockets at all, stopped both.
- **It could signal any root process,** `SystemCallFilter=@system-service` allowing `kill`.
- **A driver loaded after the service started can't be named** in `ReadWritePaths=`, which needs the path to exist at start.

Its confinement depended on settings whose interplay is easy to break with a later edit, while a dedicated user doesn't need the sandbox to keep its uid from mattering.

**Would bring it back:** a hiding step that no file permission can grant, which none so far is.
