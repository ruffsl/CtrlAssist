# Spike: hiding pads without runtime udev rules, and without root

The hiding service runs as root, writes a udev rule per pad into `/run/udev/rules.d`, reloads udev, and re-binds the pad's driver. A review found that being able to write udev rules makes it root-equivalent whatever else it is denied, since a rule can `RUN` anything as root. It proposed one static rule, shipped with the package, that hides whichever pads have a marker file the service creates and deletes. This spike asks four questions about that: does such a rule work, can hidden nodes go to a group instead of mode 0000, can a process with no privilege re-bind a driver, and if the service stays root, can it do its job with no capabilities at all.

Everything was run in NixOS VMs (QEMU with KVM, the nixpkgs test framework), on the nixpkgs the flake pins: **Linux 6.18.63, systemd and udev 262**. Nothing ran privileged on the host. It was run on 2026-11-22.

```
nix build --impure --expr 'import ./spikes/hider-rootless/test.nix { }' -L     # the four spikes, ~6 min
nix build --impure --expr 'import ./spikes/hider-rootless/sandbox.nix { }' -L  # spike 4's sandbox question alone, ~2 min
```

Each result is printed as a line starting `FINDING [spike]`. The quotes below are from the last run of each. What is here:

- `test.nix`, `test.py`: one machine with alice logged in to a graphical session (cage), in the `input` group; bob, a second user; a `ctrlassist-hider` system user and group; two fake DualSenses (`ctrlassist-fixture fake-dualsense --real`, uhid, bound to the kernel's PlayStation driver, addresses `02:ca:7e:00:00:01` and `…:02`); a uinput pad with no address (`fake-pad`, 1209:5eed); QEMU's USB tablet with serial `CTRLSPIKE1` under usbhid; and an Xbox 360 pad on the USB bus under xpad, made by `xpad.py` as a USB gadget (configfs and FunctionFS on `dummy_hcd`).
- `rules/`: the static rules under test, installed in the VM with `services.udev.packages`. `60-spike-steam.rules` does what Steam's rule does for a DualSense's hidraw node, so that the seat user can open it as on a machine with Steam.
- `hide.sh`: what the service would do, in shell, so it can be run as any user under any sandbox. `fdpass.py`: opens a node and passes the descriptor to another process.
- `sandbox.nix`, `sandbox.py`: a bare machine for spike 4's surprise.

Nothing under `crates/` or `packaging/` was changed.

## 1. The marker rule

**Question.** Does one static rule, using `TEST` with attribute substitution, hide a pad's event, js and hidraw nodes on the add that follows a re-bind and on a reconnect, with no `udevadm control --reload`, and does deleting the marker and re-binding give the pad back?

**Answer: YES**, with one addition for pads that have no driver to re-bind.

`TEST` does substitute before it calls `stat()`: in udev 262's `src/udev/udev-rules.c`, `TK_M_TEST` passes its value through `apply_format_value()` first. `udev(7)` on the same machine says only "TEST{octal mode mask}: Test the existence of a file". `%s{attr}` falls back to the parent an `ATTRS` match selected (`src/udev/udev-format.c`, `FORMAT_SUBST_ATTR`), and udev sorts a line's tokens by type with `ATTRS` before `TEST`, so the address read is the input device's.

DualSense A, as root: marker created, then the PlayStation driver unbound and bound:

```
DualSense A before: /dev/input/event7 mode=660 group=input acl=alice:rw- uaccess=yes alice=open bob=denied
DualSense A before: /dev/hidraw2 mode=660 group=ctrlassist-hider acl=alice:rw- uaccess=yes alice=open bob=denied
DualSense A before: /dev/input/js2 mode=664 group=input acl=alice:rw- uaccess=yes alice=open bob=open
marker created, no event yet: /dev/input/event7 mode=660 group=input acl=alice:rw- uaccess=yes alice=open bob=denied
DualSense A after marker + re-bind: /dev/input/event7 mode=660 group=ctrlassist-hider acl=- uaccess=no alice=denied bob=denied
DualSense A after marker + re-bind: /dev/hidraw2 mode=660 group=ctrlassist-hider acl=- uaccess=no alice=denied bob=denied
DualSense A after marker + re-bind: /dev/input/js2 mode=660 group=ctrlassist-hider acl=- uaccess=no alice=denied bob=denied
B untouched: True
A hidden on reconnect: True
A restored: True
rules in /run/udev/rules.d: [ls: cannot access '/run/udev/rules.d': No such file or directory]
```

(`uaccess` is whether the tag is in `CURRENT_TAGS`; `acl` lists named ACL entries; `alice`/`bob` is whether that user can open the node. The hidraw node is in the service's group before hiding because of the rule's raw-node line, explained under spike 2.) The directory for runtime rules never came into existence, and udev logged nothing about rules.

- **A uinput pad with no address, by model** (`model-1209-5eed`): hidden on reconnect. A change event alone is not enough without help: the first run, without the `setfacl` line, gave `uinput pad after marker + change event: /dev/input/event15 mode=660 group=ctrlassist-hider acl=alice:rw- uaccess=no alice=open`. The group and mode change, but alice's ACL stays, because the uaccess builtin only runs for nodes that still carry the tag, and nothing else takes an ACL away. With `RUN+="setfacl -b $devnode"` in the hiding branch, the change event hides it too: `uinput pad hidden by a change event: True`. Deleting the marker and sending another change event gives it back, ACL included. The old mode-0000 rule doesn't have this problem, because chmod 0000 also empties the ACL mask.
- **A USB device by serial number** (the tablet, `serial-0627-0001-CTRLSPIKE1`, re-bound under usbhid) and **the Xbox 360 pad under xpad** (`serial-045e-028e-XPADSPIKE1`): both hidden on the re-bind and given back. The xpad pad has an empty `uniq` and `ID_SERIAL_SHORT=XPADSPIKE1`. A USB HID device's `uniq` is its serial string (the tablet's is `CTRLSPIKE1`), which isn't address-shaped, so the service would key it by serial, as it does now.
- **A hostile address.** A uhid pad with `uniq` `/../../../etc/passwd` is not hidden: the marker names are flat, with a prefix (`addr-`), so a value carrying `/` names a directory that doesn't exist. udev itself lets `/` and `.` through attribute substitution (`UDEV_ALLOWED_CHARS_INPUT` is `"/ $%?,"`). Even a value that matched would only change the group of that pad's own nodes, and a seat user can do worse to a pad they created.
- **Position.** After `70-uaccess.rules`, which adds the tag, and after any `60-*` rule that opens hidraw (Steam's sets `MODE="0660"` and the tag), and before `73-seat-late.rules`, which runs the uaccess builtin on whatever still has the tag. `72-` works.

The rule that worked, `rules/72-ctrlassist-hide.rules` (with `@acl@` replaced by the acl package's path):

```
ACTION=="remove", GOTO="ctrlassist_hide_end"
SUBSYSTEM=="hidraw", GOTO="ctrlassist_hide_hidraw"
SUBSYSTEM!="input", GOTO="ctrlassist_hide_end"
KERNEL!="event*|js*", GOTO="ctrlassist_hide_end"

ATTRS{uniq}=="?*", TEST=="/run/ctrlassist/hide/addr-%s{uniq}", GOTO="ctrlassist_hide"
ENV{ID_SERIAL_SHORT}=="?*", ATTRS{id/vendor}=="?*", TEST=="/run/ctrlassist/hide/serial-%s{id/vendor}-%s{id/product}-$env{ID_SERIAL_SHORT}", GOTO="ctrlassist_hide"
ATTRS{id/vendor}=="?*", TEST=="/run/ctrlassist/hide/model-%s{id/vendor}-%s{id/product}", GOTO="ctrlassist_hide"
GOTO="ctrlassist_hide_end"

LABEL="ctrlassist_hide_hidraw"
DRIVERS=="playstation", GROUP="ctrlassist-hider", MODE="0660"
IMPORT{parent}="HID_UNIQ"
IMPORT{parent}="HID_ID"
ENV{HID_UNIQ}=="?*", TEST=="/run/ctrlassist/hide/addr-$env{HID_UNIQ}", GOTO="ctrlassist_hide"
ENV{HID_ID}=="?*", TEST=="/run/ctrlassist/hide/hid-$env{HID_ID}", GOTO="ctrlassist_hide"
GOTO="ctrlassist_hide_end"

LABEL="ctrlassist_hide"
GROUP="ctrlassist-hider", MODE="0660", TAG-="uaccess"
RUN+="@acl@/bin/setfacl -b $devnode"
LABEL="ctrlassist_hide_end"
```

`IMPORT{parent}` must have a line of its own: see "the rule the service writes today" below. The `hid-$env{HID_ID}` line (for a model-only pad's hidraw node, `HID_ID` being e.g. `0003:0000054C:00000CE6`) is in the rule but no test used it.

**Caveats.** The uhid DualSense has its address from the moment it is created. A real DualSense on USB might not: `hdev->uniq` is set from the pad's MAC by the driver, and the hidraw node may be created before that. If so, its add event would see a `HID_UNIQ` that isn't the address yet, and the node would not be hidden. I didn't check this in the kernel source, and the hiding service has the same exposure today. It needs a check with a real pad.

## 2. A group instead of mode 0000

**Question.** With hidden nodes owned by `GROUP=ctrlassist-hider`, `MODE=0660` and no uaccess tag: can a seat user who is in the `input` group open one? Does a VT switch or a user switch bring an ACL back? Can a service running as a dedicated user in that group, with no capabilities, open the hidden event and hidraw nodes and pass the descriptors on?

**Answer: YES** to the last, **NO** to the first two.

```
alice: uid=1000(alice) gid=100(users) groups=100(users),174(input)
alice (input member) can open a hidden node: False
service user sends hidden event: sender uid=999 gid=999 groups=[999] CapPrm:0000000000000000 CapEff:0000000000000000 CapBnd:0000000000000000 CapAmb:0000000000000000 | open /dev/input/event7: ok | sent
alice receives: receiver uid=1000 got /dev/input/event7 reads=100
service user sends hidden hidraw: … | open /dev/hidraw2: ok | sent
alice receives: receiver uid=1000 got /dev/hidraw2 reads=99
```

The VT and user switch: bob logged in on tty2 (`chvt 2`, getty), which put his session in front, then `chvt 1` put alice's back. Unhidden pad B's ACL went to bob and back to alice; hidden pad A's nodes had no ACL at any point:

```
bob's session in front, A: /dev/input/event7 mode=660 group=ctrlassist-hider acl=- uaccess=no alice=denied bob=denied
bob's session in front, B: /dev/input/event11 mode=660 group=input acl=bob:rw- uaccess=yes alice=open bob=open
alice's session in front again, A: /dev/hidraw2 mode=660 group=ctrlassist-hider acl=- uaccess=no alice=denied bob=denied
an ACL came back on a hidden node: False
```

That is what the source says should happen. In systemd 262, logind doesn't set ACLs itself on a seat switch: `seat_trigger_devices()` (`src/login/logind-seat.c`) sends a change event to each device that still has `uaccess` in its current tags, and udev's builtin grants the ACL. A hidden node has lost the tag, so it isn't sent one, and if it were, the marker rule would apply again.

Two things that matter for the design:

- **The service's user can't open a pad that isn't hidden** (`open /dev/input/event11: failed: Permission denied`). It doesn't need to: the client names the pad by handing it a descriptor, and once hidden, the nodes are the group's.
- **The `raw` request needs a rule line.** Today the service opens an unhidden DualSense's hidraw node as root. A rootless service needs the raw node in its group too. `DRIVERS=="playstation", GROUP="ctrlassist-hider", MODE="0660"` on hidraw nodes does it without touching seat access: `service user opens B's hidraw, not hidden: open /dev/hidraw3: ok`.

## 3. Re-binding without privilege

### 3a. Driver files given to the group

**Question.** Does a rule on driver objects that gives the `bind` and `unbind` files to the group take effect at boot and when a module is loaded later, and does it let a member with no capabilities re-bind a device, cutting off a reader?

**Answer: YES**, on the HID bus (playstation) and the USB bus (usbhid, and xpad with a gadget-made pad).

`rules/72-ctrlassist-drivers.rules`:

```
SUBSYSTEM=="drivers", ACTION!="remove", KERNEL=="playstation|xpad|usbhid", \
  RUN+="@coreutils@/bin/chgrp ctrlassist-hider %S%p/bind %S%p/unbind", \
  RUN+="@coreutils@/bin/chmod g+w %S%p/bind %S%p/unbind"
```

```
at boot: --w--w---- root:ctrlassist-hider /sys/bus/hid/drivers/playstation/bind
at boot: --w--w---- root:ctrlassist-hider /sys/bus/usb/drivers/usbhid/unbind
xpad loaded at boot: False
after modprobe xpad: --w--w---- root:ctrlassist-hider /sys/bus/usb/drivers/xpad/bind
after reloading hid_playstation: --w--w---- root:ctrlassist-hider /sys/bus/hid/drivers/playstation/bind
bob (not in the group) hides B: … | create …: failed: Permission denied | unbind …: failed: Permission denied | bind …: failed: Permission denied
service user hides B: as uid=999 CapEff: 0000000000000000 CapBnd: 0000000000000000  | create /run/ctrlassist/hide/addr-02:ca:7e:00:00:02: ok | unbind 0003:054C:0CE6.0004: ok | bind 0003:054C:0CE6.0004: ok
alice's reader (early) cut off: True [reads=105 cut_off=true]      # on B's hidraw node
alice's reader (early-ev) cut off: True [reads=52 cut_off=true]    # on B's event node
B hidden: True
service user re-binds the tablet's usbhid: … | unbind 5-1:1.0: ok | bind 5-1:1.0: ok
xpad pad 'Microsoft X-Box 360 pad' on 7-1:1.0 driver=xpad ID_SERIAL_SHORT=XPADSPIKE1 uniq=''
service user hides the xpad pad: … | unbind 7-1:1.0: ok | bind 7-1:1.0: ok
alice's reader on the xpad pad cut off: True [reads=0 cut_off=true]
xpad pad hidden by serial: True
```

A driver object gets an add event when its module registers it, which covers a module loaded later; at boot, whether the files were set by that event or by coldplug wasn't told apart. sysfs keeps the file's owner and mode until the driver is unregistered. Writing `bind` and `unbind` needs only the file's permissions, no capability, as the service user's re-binds show. The xpad pad sends no input, hence `reads=0`, but its reader was still cut off.

**Caveats.** The rule names drivers, and a rootless service can re-bind only those. Today's service re-binds whatever driver a pad has, so the list (playstation, xpad, hid-generic, usbhid, hid-nintendo, hid-microsoft, …) becomes a packaging decision, and a pad on any other driver falls back to a grab. The group's power is per driver, not per pad: a member can unbind anything on a listed driver, and usbhid and hid-generic drive keyboards and mice. That is denial of service, not escalation, but it is new.

### 3b. A synthetic event and a rule that re-binds

**Question.** Can the service trigger the re-bind from inside udev instead, by writing a synthetic uevent with an argument of its own to a uevent file a rule made group-writable?

**Answer: YES**, with no deadlock: udev settled in 0.09 s.

`rules/72-ctrlassist-rebind.rules`:

```
SUBSYSTEM=="hid", ACTION=="add", \
  RUN+="@coreutils@/bin/chgrp ctrlassist-hider %S%p/uevent", \
  RUN+="@coreutils@/bin/chmod g+w %S%p/uevent"
SUBSYSTEM=="hid", ACTION=="change", ENV{SYNTH_ARG_CTRLASSIST}=="rebind", DRIVER=="playstation", \
  ATTR{driver/unbind}="%k", ATTR{subsystem/drivers_probe}="%k"
```

The service writes `change <uuid> CTRLASSIST=rebind` to `/sys/bus/hid/devices/<hid>/uevent`:

```
uevent file: -rw-rw-r-- root:ctrlassist-hider
service user writes the uevent: sent
udev settled: True, in 0.09s
alice's reader cut off: True [reads=53 cut_off=true]
B hidden: True
udev's log:                       # nothing on timeouts, kills, failures or errors
B restored: True
```

The unbind only queues remove events for the children, which wait until the parent's change event is done. The probe asks the uhid fixture for feature reports, not udev. A real pad's driver talks to the pad, not to udev, during probe, so the same should hold, but only uhid was tried. The `synthetic_uevent` argument needs Linux 4.13 or later.

Compared with 3a, this limits re-binds to the drivers the rule names, per device, and can carry more conditions in the rule (the marker, say). The same rule shape would also let the service send change events to input devices, which 3a can't, for pads with no driver to re-bind. It costs a write to every HID device's uevent file at add, and the group can then send any synthetic event to any HID device: add, change, remove, all of which udev re-processes.

## 4. A shrunk root

**Question.** If the service stays root, can it hide and restore with the static rule (no rule writes, no reload) under `CapabilityBoundingSet=` (empty), `ProtectKernelTunables=yes` with `ReadWritePaths=` naming only the marker directory and the driver directories, `DevicePolicy=closed` with `DeviceAllow=char-input rw` and `char-hidraw rw`, and `ProtectProc=invisible`, on top of the packaged unit's hardening? Which of those break it?

**Answer: PARTLY.** Hiding and restoring work with every one of them. But that set does not confine the process as written, on systemd 262. Root with no capabilities can still reach systemd and start any unit. And a later-loaded driver's directory isn't writable.

The shrunk unit ran `hide.sh` as root with exactly that list plus the packaged unit's settings (`PACKAGED` and `SHRUNK_ONLY` in `test.py`):

```
shrunk: hide: as uid=0 CapEff: 0000000000000000 CapBnd: 0000000000000000  | create …: ok | unbind 0003:054C:0CE6.0004: ok | bind …: ok
shrunk: open: open /dev/input/event11: ok | open /dev/hidraw3: ok | open /dev/input/js4: ok
shrunk: hid=True restored=True
shrunk without char-hidraw: open: … | open /dev/hidraw3: failed: Operation not permitted | …
shrunk root opens a mode-0000 node: open /dev/input/event11: failed: Permission denied
```

So what breaks hiding: leaving `char-hidraw` out of `DeviceAllow=` (the cgroup device filter refuses), and the old mode-0000 rule (root without `CAP_DAC_OVERRIDE` is refused like anyone else, so hidden nodes must stay open to root as owner or to a group the service is in). `ProtectProc=invisible` changes nothing either way.

What it doesn't stop:

```
shrunk without the sysfs paths: hid=True restored=True
shrunk: /run and /sys as mounted: /run rw,nosuid,nodev;/sys rw,nosuid,nodev,noexec,relatime;/proc/sys ro,…
shrunk: writes a runtime udev rule: wrote
shrunk: writes other sysfs files: write /sys/bus/hid/drivers/hid-generic/bind: failed: No such device | …
shrunk: starts a unit through systemctl (private socket): escaped=True []
shrunk: starts a unit through busctl (system bus): escaped=True [o "/org/freedesktop/systemd1/job/4758"]
shrunk: signals udevd (pid 324): may signal udevd
```

- **`/run` and `/sys` stayed writable**, so the shrunk root could still write a rule into `/run/udev/rules.d`, the very thing the review is about. `sandbox.nix` isolates the cause. `ProtectSystem=strict` alone makes `/run` read-only, and adding `ReadWritePaths=/run/ctrlassist/hide` keeps it so. But adding any one of `ProtectKernelTunables=yes`, `ProtectControlGroups=yes` or `ProtectProc=invisible` makes `/run` writable again and leaves `/sys` writable:

  ```
  base + ProtectControlGroups=yes: /run/x: written | … | /sys/bus/hid/drivers/hid-generic/bind: No such device
  base + ProtectProc=invisible: /run/x: written | … | /sys/bus/hid/drivers/hid-generic/bind: No such device
  base + ProtectKernelTunables=yes: /run/x: written | … | /sys/bus/hid/drivers/hid-generic/bind: No such device
  base + (each of the other 16 settings): /run/x: Read-only file system | … | hid-generic/bind: Read-only file system
  ```

  (base: `ProtectSystem=strict`, `ReadOnlyPaths=/sys`, the marker directory and the driver directory read-write, no capabilities.) Why, from `src/core/namespace.c` in 262: those three settings imply MountAPIVFS (`namespace_parameters_mount_apivfs()`), which adds fallback entries for `/run` (`MOUNT_RUN`, a no-op when `/run` is already a mount) and `/sys` (`MOUNT_BIND_SYSFS`). The `/sys` one sorts ahead of `MOUNT_READ_ONLY`, so `drop_duplicates()` drops the read-only entry, and the journal says `/sys (read-only) is duplicate`. Every entry's path is on the deny list of the recursive read-only remount of `/`, so `/run` is skipped there, and its own read-only entry is dropped as made redundant by `/`. `MountAPIVFS=no` doesn't undo it. What does: naming them with entries that outrank those, `ReadOnlyPaths=/run` and `BindReadOnlyPaths=/sys`. The packaged unit today has `ProtectControlGroups=yes`, so its `/sys` is writable too, though its `ReadWritePaths=/run` makes the `/run` half moot.
- **A root with no capabilities is still root to systemd.** PID 1's private socket accepts any uid-0 peer, and over the system bus `StartUnit` was allowed: a job was queued and the unit ran. The unit's `RestrictAddressFamilies=AF_UNIX AF_NETLINK` leaves both open. With `AF_NETLINK` alone, both fail (`Address family not supported by protocol`). The service doesn't create Unix sockets: its listener comes from systemd and connections from `accept()`. But `udevadm settle` would no longer work, and anything else it later wants a socket for.
- **It can signal any root process** (`kill -0` on udevd succeeded). `SystemCallFilter=@system-service` allows `kill`.
- **A driver's directory has to exist when the service starts** to be named in `ReadWritePaths=`. With xpad loaded after the service started: `write /sys/bus/usb/drivers/xpad/bind: failed: Read-only file system`. The service is socket-activated and exits when idle, so the next start would see it, but a pad that brings its driver in mid-session can't be re-bound until then. Naming whole `/sys/bus/{hid,usb}/drivers` trees instead would give it every driver's `bind`, `unbind` and `new_id`.

With the corrections (`FIXED` in `test.py`: the asked list, plus `ReadOnlyPaths=/run`, `BindReadOnlyPaths=/sys`, `RestrictAddressFamilies=AF_NETLINK`), hiding still works, and the holes above close, except signals and the late driver:

```
fixed: hid=True restored=True
fixed without the sysfs paths: hide: … | unbind …: failed: Read-only file system | …
fixed: writes a runtime udev rule: … /run/udev/rules.d/99-x.rules: Read-only file system
fixed: writes other sysfs files: write /sys/bus/hid/drivers/hid-generic/bind: failed: Read-only file system | write /sys/bus/hid/drivers/playstation/new_id: failed: Invalid argument
fixed: starts a unit through systemctl (private socket): escaped=False [… Address family not supported by protocol]
fixed: starts a unit through busctl (system bus): escaped=False [… Address family not supported by protocol]
fixed: signals udevd (pid 324): may signal udevd
```

(`Invalid argument` on `new_id` means the write reached the kernel: the whole driver directory is writable. Whether `ReadWritePaths=` can name only the `bind` and `unbind` files wasn't tried.)

## Surprising

- **The hidraw line of the rule the service writes today never matches.** udev sorts each line's tokens by type, and `ENV` matches come before `IMPORT{parent}` (`sort_tokens()` in `udev-rules.c`). So in `SUBSYSTEM=="hidraw", KERNELS=="…", IMPORT{parent}="HID_UNIQ", ENV{HID_UNIQ}=="<address>", …` the address is compared before it is imported. Written before the static rule's own import (as `71-`), the one-line form left the DualSense's hidraw node tagged and open to alice: `today's hidraw line, one line: /dev/hidraw2 … acl=alice:rw- uaccess=yes alice=open`. Split in two lines it matched: `… acl=- uaccess=no alice=denied`. (That line reports `denies the node: False` only because the static rule's later raw-node line put the mode and group back; the tag and ACL show that it matched.) On a machine with Steam's rules, a DualSense hidden today keeps its hidraw node open to the seat user. This is a bug in `crates/ctrlassist-hider/src/hide.rs` (`rule_text`), whatever is decided here.
- **js nodes are world-readable on a stock machine** (`50-udev-default.rules`: `MODE="0664"`): bob could read alice's pad's js node throughout. The hiding rule's `MODE="0660"` closes that for hidden pads, as mode 0000 did.
- **A marker does nothing until the pad's next event.** The order is marker first, then the re-bind. Deleting a marker leaves a pad hidden until it is re-bound or reconnects.
- **`ProtectKernelTunables=`, `ProtectControlGroups=` and `ProtectProc=` weaken `ProtectSystem=strict`** on systemd 262, as above. I don't know if upstream has changed this. It could be NixOS-specific, though nothing in the code path is.

## Not tested

- A real DualSense on USB and on Bluetooth (the hidraw address timing above), and any distribution other than NixOS.
- Older udev and systemd. Not chased, as asked. I believe the mechanisms used (`TEST` with substitution, `TAG-=`, `IMPORT{parent}`, `RUN`, driver-object events) are all much older than 262, but I didn't check. The split between `TAGS` and `CURRENT_TAGS`, which `TAG-=` and the seat-switch behavior rely on, came with systemd 247; before it, `TAG-=` removed the tag outright, which should hide the same. The sandbox interaction in spike 4 has to be checked on whatever version is the floor.
- A rootless service sending change events to input devices (pads with no driver to re-bind): the change path was tried as root only. It needs the input devices' uevent files given to the group, as 3b does for HID devices.
- `udevadm settle` as the service's user; the real service would watch udev's monitor instead.
- The real hiding service under any of this. Everything was prototyped in shell.

## Recommendation

**Drop root, with a dedicated user and group.** Every step of hiding worked with no capabilities at all: markers in a directory the service owns, re-binds through driver files given to its group (or through a synthetic event, 3b), hidden nodes and the DualSense's raw node opened through the group, descriptors passed to the daemon. The seat user, in the input group or not, and across VT and user switches, got nothing back. The shrunk root also hides correctly, but it stays root to systemd unless it can create no Unix sockets at all. The obvious sandbox for it left `/run` and `/sys` writable without any warning, on the systemd under test. Keeping it confined depends on settings whose interplay is easy to break with a later edit. A dedicated user doesn't depend on that sandbox to keep its uid from mattering.

Between 3a and 3b, 3a is simpler and was tried on both buses. 3b confines re-binds to the rule's conditions and also gives a way to send change events, which pads with no driver need. Either needs a decision on the driver list.

What would change, if this is taken up:

- `packaging/udev/`: the hiding rule above as `72-ctrlassist-hide.rules`, with paths for distributions (`/usr/bin/setfacl`); the driver rule (3a) or the uevent and re-bind rule (3b); and the raw-node line.
- `packaging/systemd/ctrlassist-hider.service`: `User=` and `Group=ctrlassist-hider`, `CapabilityBoundingSet=` empty, no `ReadWritePaths=/run`, and the marker and record directories as `RuntimeDirectory=ctrlassist/hide ctrlassist/hidden` with `RuntimeDirectoryPreserve=yes`, so markers outlive an idle exit and `reconcile` still finds them. Then `DevicePolicy=closed` with `char-input rw` and `char-hidraw rw`, and `RestrictAddressFamilies=AF_NETLINK`. If `ProtectKernelTunables=`, `ProtectControlGroups=` or `ProtectProc=` stay, add `ReadOnlyPaths=/run` and `BindReadOnlyPaths=/sys`, as defense in depth now rather than the whole defense. A sysusers.d file for the user and group, and the NixOS module, deb, rpm and Arch packaging to match.
- `crates/ctrlassist-hider/src/hide.rs`: write and delete marker files (`addr-<uniq>`, `serial-<vid>-<pid>-<serial>`, `model-<vid>-<pid>`, and `hid-<HID_ID>` for a model-only pad's hidraw) instead of rule files. No `udevadm control --reload`. Wait on udev's monitor rather than `udevadm settle`. Drop the saved modes and owners, which nothing changes any more. Pads with no driver to re-bind need a change event the service can send (see above). A record keeps what it records, minus the node modes.
- `crates/ctrlassist-hider/src/pad.rs`: re-bind only on drivers the rule names, and refuse others as unsupported, so the daemon grabs them instead.
- `docs/architecture.md`: the hiding section (what the service is allowed, what a marker is, the driver list), and the platform notes on `TEST`, the token order and the systemd sandbox.

Minimum version: what was shown is for systemd and udev 262 on Linux 6.18 (3b needs Linux 4.13 or later). Nothing found here is known to need a newer systemd, but no version other than 262 was run.
