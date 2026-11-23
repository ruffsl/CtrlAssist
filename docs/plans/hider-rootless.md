# The hiding service without root

## Goal

The hiding service runs as root today. It writes a udev rule per pad into `/run/udev/rules.d`, has udev reload, re-binds the pad's driver through sysfs, and opens nodes it has set to mode 0000. Being able to write udev rules is root whatever else the unit denies, since a rule can `RUN` any program as root; and the unit leaves all of `/run` writable besides. The workspace review ([review.md](review.md), "The hiding service without root") found that, and found two high-severity gaps in how the service decides what a descriptor names (H-1, H-2) and several ways any local user can stop it serving (H-3 to H-6).

This plan makes the service an unprivileged process: a dedicated user with no capabilities, whose whole power is the files a static, shipped udev rule gives its group. It fixes H-1 and H-2 in that shape, as the review decided, and the medium and low findings with them, so none is fixed twice.

## Decisions

Made by the maintainer on 2026-11-22 (review.md's Decisions), after the spikes in `spikes/hider-rootless/`, which this plan replaced (in the history at `934c482`).

- **The service drops root.** It runs as the user and group `ctrlassist-hider`, with no capabilities at all. A root kept and shrunk was set aside: on systemd 262 three of the hardening settings it would lean on (`ProtectKernelTunables=`, `ProtectControlGroups=`, `ProtectProc=`) silently leave `/run` and `/sys` writable under `ProtectSystem=strict`, and a root with no capabilities still started a unit through systemd's private socket and over the system bus. A dedicated user doesn't need the sandbox to keep its uid from mattering. Both designs, and why, are in [the idea doc](../ideas/hider-rebind-options.md).
- **One static rule, shipped with the package, keyed on marker files** the service creates and deletes, in place of rules written at runtime. udev's `TEST` substitutes before it calls `stat()` (`TK_M_TEST` passes its value through `apply_format_value()` in udev 262), so a line can test for a file named after the device's own attributes; the rule is evaluated at every event, so no reload is needed. The service writes only file names, built from strings it has checked.
- **Hidden nodes belong to the service's group,** `GROUP="ctrlassist-hider"`, `MODE="0660"`, with the seat-access tag removed. A group needs no capability where mode 0000 needed `CAP_DAC_OVERRIDE`. The spike showed a seat user who is also in `input` can't open them, and that no ACL comes back through a VT or a user switch.
- **A pad's driver is re-bound by asking udev, a pad at a time.** The service writes a synthetic event, `change <uuid> CTRLASSIST=rebind`, to the `uevent` file of the pad's HID device or USB interface; a shipped rule makes that file writable by the group for devices already known to be pads, and another does the unbind and the probe when it sees the event. The service can then touch nothing but pads, and needs no list of drivers. The other way that worked, driver files given to the group for a listed set of drivers, reaches every device such a driver has, keyboards among them; it is kept in [the idea doc](../ideas/hider-rebind-options.md).
- **The oldest supported distribution may move up** if this needs it. It doesn't, as far as is known: see [Minimum versions](#minimum-versions).

## Design

### The user, the group and the directory

- **The user and its group** come from a `sysusers.d` file in the packages (`u ctrlassist-hider - "CtrlAssist hiding service" - -`), which the deb, rpm and Arch packages have `systemd-sysusers` apply at install, and from `users.users` and `users.groups` in the NixOS module. The user has no home and no shell. It is created whether or not hiding is turned on, since the rule names the group.
- **The marker directory is `/run/ctrlassist/hide`,** the unit's `RuntimeDirectory=` with `RuntimeDirectoryPreserve=yes` and mode 0700, owned by the service's user. A plain `RuntimeDirectory=` is deleted when the service stops, which would lose what a crash left hidden; preserved, it lasts until the unit's own `ExecStopPost=` puts everything back, and `/run` being emptied at boot bounds the worst case. udevd, as root, reads it whatever its mode.
- **The markers are the only record.** Today's `/run/ctrlassist/hidden` records go: a marker is created before the re-bind that makes it take effect, so whatever a crash leaves hidden is named by a marker, and undoing it needs nothing else (see [Reconcile and the sweep](#reconcile-and-the-sweep)).

### The static rule

`72-ctrlassist-hide.rules`, at 72 for the reason today's runtime rules are: after `70-uaccess.rules`, which tags pads for seat access, and after any `60-*` rule that opens hidraw (Steam's sets `MODE="0660"` and the tag), and before `73-seat-late.rules`, which grants what the tag asks for. It does four things, all from the spike's rule text that worked, with the changes below:

1. **The re-bind request.** On a `change` event of a device in the `hid` or `usb` subsystem whose `SYNTH_ARG_CTRLASSIST` is `rebind`: `ATTR{driver/unbind}="%k", ATTR{subsystem/drivers_probe}="%k"`. The probe, not a bind to the same driver, so whichever driver the kernel would pick at a plug-in takes the pad again.
2. **Which devices may be asked.** On the event node of a pad (`ID_INPUT_JOYSTICK` and none of the keyboard, key, mouse, touchpad or tablet classes, as the service's own check), the `uevent` file of its HID device, or, with none, of its USB interface, is given to the group and made group-writable (`chgrp` and `chmod g+w`, run by udev). sysfs keeps a file's owner and mode as long as the device exists, and a HID device or USB interface outlives the re-bind of its driver.
3. **The raw request's nodes.** `DRIVERS=="playstation"` hidraw nodes get `GROUP="ctrlassist-hider", MODE="0660"`, beside whatever seat access they have. A stock hidraw node is root's alone, so this opens nothing to the seat user; the service needs it to open an unhidden DualSense's raw node without root.
4. **Hiding by markers.** An event, js or hidraw node whose pad has a marker gets `OWNER:="root", GROUP:="ctrlassist-hider", MODE:="0660", TAG-="uaccess"`. The final assignments (`:=`) are a change from the spike, so that a later rule can't open a hidden node again (H-8).

`IMPORT{parent}` has a line of its own wherever an imported value is matched, since udev compares a line's `ENV` keys before it runs the line's import (H-15). Paths in `RUN` are `/usr/bin/chgrp` and `/usr/bin/chmod` in the packages, rewritten to the store's in the Nix package.

### Which pads, and how each is named

A marker is a file whose name says how the rule finds the pad. Every name has a prefix, so an attribute that is empty, or carries `/` or `..` where it shouldn't, names a file that doesn't exist; and a match only changes the group of the matching device's own nodes. The service writes, for each pad it hides, every marker below that applies:

| Marker | Written when | Matches |
|---|---|---|
| `addr-<uniq>` | the input device's `uniq` is an address (`aa:bb:cc:dd:ee:ff`, lowercase) | event and js nodes, through `ATTRS{uniq}` |
| `serial-<vendor>-<product>-<serial>` | no address, and udev gives a USB serial number | event and js nodes, through `ID_SERIAL_SHORT` |
| `hid-<HID device>` | the pad has a HID device | its event, js and hidraw nodes, through the HID device's kernel name (`%b` after `SUBSYSTEMS=="hid"`) |
| `uniq-<HID_ID>-<HID_UNIQ>` | the HID device's own `HID_UNIQ` is set and well-formed | hidraw nodes, through `IMPORT{parent}` |
| `port-<phys>` | no address and no serial, for event and js nodes; no `HID_UNIQ`, for hidraw | nodes whose `phys` (input device) or `HID_PHYS` (HID device) is that USB port path |

What this means for each kind of pad:

- **An address** (a DualSense or DualShock 4 on USB or Bluetooth; anything on Bluetooth) names the pad wherever it reconnects. Its event and js nodes are matched by it.
- **A HID pad's hidraw node is matched by its HID device,** not by an address. The HID device keeps its kernel name through a re-bind of its driver, so the node the re-bind makes is hidden whatever the pad's address is doing. This is the fix for review.md's two hidraw notes: a HID pad on USB has no address in its HID device's properties, so its hidraw line went by model and hid every pad of that model; and a real DualSense on USB has its hidraw node made before the driver sets the address, so a line keyed on the address could miss it. Neither matters to a marker keyed on the HID device. On Bluetooth the HID device carries the address from its creation, so `uniq-` covers the hidraw node of a pad that reconnects. On USB, `port-` covers one that reconnects to the same port. **One that reconnects to another port has its hidraw node open to whatever opens hidraw for the seat user (Steam's rules) until it is shown again:** the HID device is new, its address isn't in its properties, and its port is another. Closing that needs a re-bind as it is opened again, which the daemon would have to expect (see [What the daemon needs](#what-the-daemon-needs)).
- **A serial number** names a USB pad with no address (an Xbox pad under `xpad`) wherever it reconnects. Clones that share a serial share its fate while one is hidden, as they do today.
- **A pad with neither** is named by its HID device and its port, so it is hidden through the re-bind and through a reconnect to the same port. Today's model-only rule, which hid every pad of the model on every seat at its next event, is gone (H-8).
- **A pad with none of these** (no HID device and no usable `phys`) isn't hidden: the service refuses it as unsupported and the daemon grabs it.

A marker that matches another device than the one it was made for (a forged twin with the pad's address, clones with one serial, a pad plugged into the hidden pad's port while it is away) hides that device's nodes at their next event. The sweep below shows them again once no marker names them.

### Asking udev to re-bind

- **Hiding** creates the pad's markers, then writes the request to the `uevent` file of the pad's HID device or USB interface, the device recorded when the pad was checked. The re-bind removes the pad's nodes, which cuts off whoever had them open, and the nodes it makes again are hidden by the rule as they are created.
- **Showing a pad again** deletes the markers no other hidden pad uses, then sends the same request.
- **What replaces `udevadm settle` and the reload.** There is no reload: the rule never changes. In place of settle, the service watches udev's monitor (`AF_NETLINK`, which an unprivileged process may read) from before it writes the request, and waits for two things in turn: the `change` event of the device it asked, carrying the request's own UUID, which udev broadcasts once the rule has unbound and probed; then the `add` of each event node the kernel has under that device by then, which udev broadcasts once the rule has applied the markers to it. udev orders a parent's event before its children's, so nothing is missed between the two. If after the first the device has no driver, or no event node, the probe made no pad, and the request fails. No time limit: the wait ends on udev's events, or on the client hanging up.
- **The wait holds no lock** over the service's other connections; a pad being hidden is marked pending, and a second hide of it is refused as busy.

### Pads with no driver to re-bind

A pad whose event node has no HID device and no USB interface with a driver (a uinput pad another program made) can't be cut off from whoever has it open, and today's way of hiding it, a change event and the nodes' modes put back by hand, needs `CAP_FOWNER` and `CAP_CHOWN`. **The service refuses it as unsupported,** and the daemon, which grabs any pad the service refuses, grabs it. A grab silences readers that already had it open, which hiding by a change event didn't do. That is today's daemon behavior on a refusal, so no daemon change is needed. The daemon's `hidden` flag for such a pad becomes false, which is true: it is grabbed.

### The raw request

`raw` names an unhidden pad by its event node, as today, and is now checked as `hide` is (below). The service opens the hidraw node **of the HID device of the pad presented**, not of the first pad with the same identity (H-2), and only for the PlayStation driver, as today. It can open it because of the rule's raw-node line. `raw of` opens the hidraw node of the HID device a hidden pad has now.

### Checking a descriptor, and finding the pad again

- **A descriptor proves access only if it was opened for reading.** The service checks it with no call that could block on another process's file system: `fcntl(F_GETFL)` must show no `O_PATH` and a readable access mode; `statx` with `AT_STATX_DONT_SYNC` (which a FUSE file answers from its cache) must show a character device of the input major; and `EVIOCGVERSION` must succeed on it, which an `O_PATH` descriptor can't do. Then, as today, udev must classify it as a joystick and nothing a keystroke or pointer could come from, tagged for seat access. (H-1)
- **The pad's seat must be one where the caller's session is in front:** its `ID_SEAT`, `seat0` when unset, against the seats the login library says the caller is in front on. (H-1)
- **The service acts on the device it checked.** It records the HID device's or USB interface's sysfs path and the input device's, and the re-bind, the opening of the new event node, `raw of`, `reopen` and showing the pad again all act on that path: the new event node is the joystick event node under the recorded device, the same one by order if it has several. Nothing is found again by vendor, product and address while the recorded device exists. (H-2)
- **A pad that reconnected is found again only once its recorded device is gone,** and then only among event nodes the rule has hidden, that are joysticks, on the same bus, with the same vendor and product and seat, under the same kind of device (`/sys/devices/virtual` or not, which a uhid device can't choose), that one of the entry's markers names, and that no other hidden pad has. What is left: a pad that is itself a uhid device (on Bluetooth, through BlueZ) has a twin indistinguishable in sysfs, so a twin a seat user makes while the pad is away can be taken for it on its return. Only the user at seat0 can open `/dev/uhid` (the packaged rule gives it to the seat's user), so this is the seat user's own process against their own daemon, which the service doesn't defend against.

### Who is served, and when

- **A connection is permitted** once its user is found in front on a seat, as today, and stays so. A permitted connection may always restore, say the version, and `reopen` and `raw of` what it has hidden: a user who leaves the foreground keeps what they have, as the architecture says.
- **`hide` and `raw` need the caller in front now,** on the pad's seat. A user switched away from can take nothing new. (H-3)

### Connections

- **One thread per connection,** each receiving, checking and replying for its own client, with the shared state (the hidden pads, the markers) behind one lock that no thread holds while it waits on a client or on udev. A client that never reads its replies blocks only its own thread at `sendmsg`; one that passes a descriptor whose last close blocks (a FUSE file's flush, a lingering TCP socket) blocks only its own thread, since every descriptor received is closed there, the checked ones included. (H-4, H-5)
- **Unpermitted clients are still answered,** so a daemon that started before its session was in front keeps its connection and is served once it is, as today; it costs them only their own thread.
- **At most eight connections per user.** A ninth is accepted and closed at once. The daemon holds one. (H-6)
- **`accept` failing is not fatal.** Out of descriptors, the service stops accepting until a connection closes, instead of exiting and showing every hidden pad mid-session. (H-6)
- **Started on demand, it exits when the last connection closes,** as today; the main thread decides, so a client waiting to be accepted is accepted rather than dropped.

### Reconcile and the sweep

- **Reconcile,** when the service starts and from the unit's `ExecStopPost=` however it stopped, deletes every marker, then shows again every pad the rule has hidden.
- **The sweep:** a pad is hidden by the rule exactly when its event node belongs to the service's group, which nothing else sets. After a pad is shown again, and at reconcile, the service sends the re-bind request for each such pad that no hidden pad's record holds and no remaining marker names. That is how a forged twin, a clone or a pad plugged into a hidden pad's port comes back. It doesn't wait for those re-binds.

### The unit's sandbox

`User=` and `Group=ctrlassist-hider`; `CapabilityBoundingSet=` and `AmbientCapabilities=` empty; `NoNewPrivileges=yes`; `ProtectSystem=strict` with the marker directory the only writable path under `/run`; `/sys/devices`, where the `uevent` files are, the only one under `/sys` (`ProtectKernelTunables=yes` makes `/sys` read-only, and the spike's `BindReadOnlyPaths=/sys` keeps it so on systemd 262, where that setting alone doesn't); `DevicePolicy=closed` with `DeviceAllow=char-input rw` and `char-hidraw rw`; `RestrictAddressFamilies=AF_NETLINK`, since the service creates no socket but udev's monitor (its listener comes from systemd, connections from `accept`); `ProtectProc=invisible`, `ProcSubset=pid`, `PrivateIPC=yes`, `RemoveIPC=yes`, `UMask=0077`; and the packaged unit's other settings. Each is checked in the VM by the service still hiding and restoring. The network namespace stays shared: udev's events arrive over netlink in the host's. `ReadWritePaths=/run` goes. These are defense in depth now: the user's permissions are the boundary.

### What a compromise yields

Not root. The service's user can open the nodes of pads that are hidden and the raw nodes of PlayStation pads, hidden or not, which means reading their input and writing them output and feature reports; have udev re-bind the driver of any pad, and send any pad's HID device or USB interface any synthetic event, which udev processes as root through the stock rules: denial of service on pads, not keyboards or mice; and hide any pad by writing markers. It can write nothing outside its own directory and the pads' `uevent` files, and has no capability to lose.

### Minimum versions

What is used: `TEST` with substitution, `IMPORT{parent}`, `TAG-=`, final `:=` assignments, `%b` in `RUN` formatted when its line is applied (udev since its rules engine was rewritten in 243); `CURRENT_TAGS` (247), which the seat-switch behavior the spike observed relies on; synthetic uevent arguments (Linux 4.13); `ProtectProc=` and `ProcSubset=` (systemd 247), `PrivateIPC=` (248), `RuntimeDirectoryPreserve=` (235), `sysusers.d` (215). So **systemd 248 and Linux 4.13**: Debian 12 (252), Ubuntu 22.04 (249), Fedora and Arch are all above it. Only 262 is run in the VM; the rest is from the versions each feature arrived in, not tried.

## The findings, one by one

- **H-1 (an `O_PATH` descriptor names any pad; the seat isn't checked): closed** by the descriptor checks and the seat check above. Shown in the VM by an `O_PATH` descriptor refused for `hide` and `raw`.
- **H-2 (a pad found again by forgeable identity): closed** by acting on the recorded device, and re-identifying only after it is gone, under the conditions above, with the uhid-on-Bluetooth residual stated. Shown in the VM by a uhid twin with another pad's vendor, product and address: hiding the twin returns the twin's node and leaves the real pad visible, and the real pad can still be hidden.
- **H-3 (powers kept after leaving the foreground): closed** for new requests: `hide` and `raw` need the caller in front now. Keeping what was hidden, and `reopen` and `raw of` of it, stay, by the architecture's own promise.
- **H-4 (hanging the service by not reading replies): closed** by a thread per connection. Shown in the VM by a client that sends a thousand requests and never reads, while another is served.
- **H-5 (a descriptor whose close blocks): closed** by closing descriptors on the client's own thread and checking them without calls that wait on another file system. Not shown with a FUSE file in the VM; the thread structure is what H-4's test shows.
- **H-6 (exhausting descriptors makes it exit): closed** by the per-user cap and `accept` failures not being fatal. Shown in the VM by many idle connections from one user while another user's client is served.
- **H-7 (the uinput and uhid grant): not closed here.** It is the daemon's access, not the service's; closing it means the daemon receiving those descriptors through the service, a protocol and daemon change and the service's user needing uinput and uhid. The maintainer's to decide.
- **H-8 (hiding undone by later rules; model rules spill over): closed** by final assignments and per-pad markers, with the spill-over that remains (twins, a pad in a hidden pad's port) undone by the sweep.
- **H-9 (65,535 HID devices): closed.** A HID device's name is checked as the kernel forms it, with four or more instance digits, and it is no longer written to a driver file: the rule uses `%k`.
- **H-10 (sockets leak into `udevadm` children): no longer applies,** the service running no programs; and closed anyway: the listener is made close-on-exec, connections are accepted with `SOCK_CLOEXEC`, and the activation variables are unset.
- **H-11 (the unit doesn't contain a compromise): no longer applies** in its root form; the sandbox above is added as defense in depth.
- **H-12 (`--listen` sets the mode by path): closed** by binding under a umask that gives the mode, with no `chmod` after.
- **H-13 (truncated requests accepted): closed:** a message longer than a request is refused.
- **H-14 (tokens wrap): closed:** a token in use is never handed out again; and a token no longer names a file.

## The wire protocol

Unchanged: the same requests, replies, refusals and sizes, so today's daemon works against the new service. Two things the client library changes, neither visible on the wire: `receive` reports a message's whole length, so a reply longer than a reply is a protocol error rather than its first six bytes; and `Refusal::Unsupported`'s text no longer speaks only of raw nodes, being also the answer for a pad with no driver to re-bind.

**`dislodged` keeps its meaning:** on a hide, that the service re-bound the pad's driver, and will re-bind it again to show it. Every hide that succeeds now re-binds, so it is always true; a pad that couldn't be re-bound is refused instead. `reopen` still replies `dislodged`, as today, and the daemon still ignores it there.

### What the daemon needs

Nothing for this plan. For the one case left open (a USB HID pad that reconnects to another port while hidden), the service could re-bind it as it is opened again, if the daemon then expected the pad to go and come back once more, as it does when it lets a pad go. That needs `reopen`'s `dislodged` to mean "re-bound now" and the daemon to act on it, and is not done here.

## Steps

1. **The plan and the idea doc.** Verified by the docs gates.
2. **The static rule, the user and the unit, and the service on them.** The rule file, the `sysusers.d` file, the unit, the NixOS module and package, the deb, rpm and Arch contents and scripts; in the service, markers in place of rule files and records, the re-bind request and the wait on udev's monitor, pads with no driver refused, reconcile and the sweep, and the recorded device acted on. The spike directory goes, its durable findings moved into the architecture and the idea doc. Verified by the VM checks `hider` and `daemon`, extended: the service's uid isn't 0 and it has no capabilities; nothing appears under `/run/udev/rules.d`; a pad is hidden and shown again through a re-bind and through a reconnect; the seat user, in `input` too, can't open hidden event or hidraw nodes; the Xbox 360 pad the spike made as a USB gadget (`xpad` on `dummy_hcd`) is hidden and shown again by the re-bind request; a pad with no driver is refused and grabbed by the daemon.
3. **H-1 and H-2:** the descriptor checks, the seat, re-identification only once the recorded device is gone. Verified in the VM: an `O_PATH` descriptor refused; a forged twin not taken for the real pad.
4. **H-3 to H-6:** a thread per connection, new requests only from the foreground, descriptors closed on their own thread, the per-user cap, `accept` not fatal. Verified in the VM: a client that never reads and many idle connections don't stop another client being served.
5. **H-8 to H-14,** whatever the steps before left. Verified by the hider's unit tests and the VM checks.
6. **The docs:** the architecture's hiding section and its platform notes, the packaging README, the guide's pages that say the service runs as root or what installing sets up, and the justfile's dev recipe for the service. Verified by the docs gates.

What only hardware can show, after the VM checks:

- **A real DualSense on USB:** hidden and shown again, its hidraw node hidden through the re-bind though its address comes after the node; reconnected to the same port and to another, which says whether the open case above is real with Steam running.
- **A pad on Bluetooth** (a DualSense or DualShock 4 through BlueZ): hidden, reconnected, shown again, with Steam running.
- **An Xbox pad on USB under `xpad`,** for the re-bind request at a USB interface on a real pad, whose probe talks to the pad.

## State

- **The plan and the idea doc: done.**
- **The static rule, the user and the unit, and the service on them: done.** The hider check passes with the service as its own user (uid 999, every capability set empty, read from `/proc`); no runtime rule directory ever appears; a uhid pad is hidden and shown again through the re-bind request and through a reconnect, with its event, js and hidraw nodes closed to a seat user who is also in `input`; a DualSense's raw node, which a Steam-like rule opens, is closed while it is hidden; the Xbox 360 pad made as a USB gadget is hidden and shown again by the request at its USB interface, its open reader cut off; a uinput pad is refused; a killed client and a killed service leave nothing hidden. The daemon check passes with the module's user and rule: its uhid pad hidden, its uinput pad grabbed, through a reconnect, a live update, a stop, a kill and a start by itself, and the DualSense copied through the service's raw request. The hider crate's unit tests pass; its tests against a stand-in service can't bind their socket in this worktree, whose path is longer than a socket address allows, and weren't run.

- **H-1 and H-2: done.** The hider check now passes a descriptor opened `O_PATH` to `hide` and to `raw` from a client of its own speaking the wire format, and each is refused as no pad with nothing passed back; hides a uhid twin with the real pad's vendor, product and address, and gets back the twin's own node, the real pad untouched and still to be hidden after; and refuses the seat user a pad udev puts on seat1, though she can open it through the input group.

## Deviations

- **Finding a pad again after a reconnect landed with the mechanism,** not with H-1 and H-2's step: the reconnect is part of what the mechanism's checks show, and the service acts on the recorded device from the start. H-1 and H-2's step keeps the descriptor checks, the seat, and the forged-twin check.
- **The architecture's hiding section, and the spike's findings moved into its platform notes, changed with the mechanism,** so that it says what the code does at each commit; the docs step keeps the guide and the packaging README.
- **H-3 closed with H-1's seat check,** not with the robustness step: asking that the caller be in front, now, on the pad's seat is the same check as asking it for new requests only.
