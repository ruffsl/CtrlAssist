# A re-bind known by what it leaves in place

## Goal

Hiding a pad, and showing it again, re-binds its driver: the pad's nodes go and new ones come. Two pieces of machinery grew around that over three rounds of review and fixes, and each round's review found faults in what the round before had added ([the review](review.md), the second, third and fourth).

- **The daemon counts.** It must not publish a re-bind as a pad disconnecting and connecting, nor take the pad's return for a pad plugged in, which would start again a managed profile the user had just stopped. So it expects each going and coming back before the request that causes it, has the hiding service count its re-binds in every reply, settles hides given up on tasks of their own, and checks for a count left over.
- **The hiding service takes turns.** So that one re-bind isn't taken for another's, each device's re-binds wait their turn, and the sweep's run on threads of their own.

The fourth review found that the turns answer a fault that could not happen, udev already running one device's events in order; and that the count is not needed, a re-bind being recognizable by what it never removes. The goal is to take both out.

## Decisions

Made by the maintainer on 2026-11-23, on the fourth review's recommendations.

**The daemon knows a re-bind by the pad's device, and counts nothing.** A re-bind removes a pad's input nodes and never the device whose driver is re-bound: its HID device, or for a pad with none its USB interface. That device, with what tells one enumeration of it from the next, is the pad's *anchor*. The hiding service already records it, to act only on the device it checked. The book's rule becomes:

- A pad's node goes and its anchor is still there: the pad is away for a moment. Nothing is published, and it is still known as connected.
- A node comes on the same anchor: the pad is back. It is not a plugging in, so it prompts no start.
- The anchor itself goes: the pad is unplugged, and is published as gone.
- A node comes on an anchor not seen before: a pad is plugged in.

This holds whoever re-bound the pad and however many times: the service for a hide or a showing again, its sweep, a service from before, or anyone else. It needs no count, no word from the service, and no argument from the order udev sends things in beyond a device's removal coming after its children's.

**A hide is given up by hanging up,** as before the count: the service shows again what a connection hid when the connection closes. Nothing needs to be told what that took.

**The service's re-binds take no turns.** Done still needs the request's own answer, with the checks of state that fail early and that conclude after the monitor overran. udevd runs an event only after every earlier one for the same device, its parents and its children, so a later request's answer comes after every earlier request of that device has run; that is to be read in udevd's source and shown in the VM by two requests written back to back, before the turns go. The sweep sends its requests and waits for none, as the reconcile at start does.

**`cancel` and the counts leave the wire,** nothing being released that speaks them: the request, the refusal it brought, the bits in a reply's flags, the mark in the release's answer, and the service's and the client's code for them.

**The findings of the fourth review that this doesn't remove are fixed with it:** 4H-1, 4H-3, 4M-2, 4M-3's two that aren't of the count, and the device crate's 4D-1, 4D-2 and 4D-4. 4D-3 waits on hardware.

**A re-bind is known by its request** (decided after the fifth review, which found the rule above wrong for a pad on a wireless receiver, whose input goes when the pad is switched off while the receiver, its anchor, stays). The hiding service asks a re-bind by a change event on the anchor that it marks as its own, and udev sends that event on before the removals it causes. The hotplug watch hears it. So the first line of the rule becomes: a pad's node goes after its anchor was so marked, and the anchor is still there: the pad is away. A node that goes with no such mark has left, and is published, whatever became of its anchor. The mark is cleared when the pad's nodes are back. A re-bind by anyone but the service is then published as a pad leaving and one connecting, which is what it is.

## What is lost and what is not known

- **A re-bind no driver comes back from** leaves the pad away, and so published as connected, until its anchor goes. The count had the same gap, recorded as accepted.
- **A pad unplugged in the moment of a re-bind:** its anchor's removal says so, where the count could leave the pad published as there until plugged in again. A gain.
- **Bluetooth.** A pad BlueZ presents through uhid, or through the kernel's HIDP, keeps its HID device through a re-bind as far as the driver model says: unbinding a driver doesn't remove the device. The hiding service leans on the same, and its VM check runs it for USB and uhid. Not seen on a real pad over Bluetooth; among the hardware checks.
- **Two pads that share a row of the book** (no address, serial or port to tell them apart) were miscounted before; by anchor each node has its own, which should tell them apart. To be confirmed as it is built.

## The anchor, by kind of pad

The device crate reads a node's anchor as the hiding service finds the device it records and re-binds: the nearest HID device above the node, or with none the nearest USB interface, with the `USEC_INITIALIZED` of the nearest USB device at or above it, where there is one. The two must agree: an anchor below the device the service re-binds would go with each re-bind.

- **A HID pad on USB** (a DualSense, a DualShock 4): its HID device, under its USB interface, stamped by its USB device. The HID device's name carries an instance number the kernel never hands out twice, so the stamp only repeats what the place says.
- **A HID pad over Bluetooth through uhid** (BlueZ): its HID device, under `/sys/devices/virtual/misc/uhid`, with no stamp. BlueZ destroys the uhid device as the pad disconnects, which removes the HID device.
- **A HID pad over Bluetooth through the kernel's HIDP:** its HID device, under the adapter's connection, stamped by the adapter's USB device where the adapter is on USB; the connection and the HID device go as the pad disconnects.
- **A USB pad with no HID device** (xpad): its USB interface, whose place is its port's, stamped by its USB device: another pad plugged into that port is another enumeration.
- **A uinput pad another program made:** no anchor. Nothing re-binds it (the service refuses it, and the daemon grabs it), and its event node is what goes when it does, so its node going is its unplugging, as before.
- **A pad with several input nodes** (a DualSense's touchpad and motion sensors): the daemon takes in only a pad's joystick nodes, and those of one HID device share its anchor, each node away and back on its own.
- **A node whose anchor can't be read** (it went as it was read, or its USB device has no stamp yet): its own anchor, as a uinput pad's.

The hotplug watch hears, beside input nodes, the removal of HID devices and USB interfaces, filtered by udev; a removal unplugs every pad anchored at or under the device removed, once. A pad present as the daemon starts has its anchor read with it; one away then has none known, and its return is a pad plugged in, as it would be had it been there a moment earlier.

## Steps

Each is a branch of its own, verified by the suites that make no pads, by the whole suite while Steam is closed, and by the VM checks.

1. **The daemon.** The anchor, as a function in the device crate that reads it for a node as the service does; the hotplug watch hearing a HID device's and a USB interface's removal as well as input nodes'; the book by anchor; out with the count, the expectations, the guard and its flush, the settling of hides given up, and the daemon's use of `cancel` and of counts; showing a pad again still waits for its writer and then asks the restore and reads its reply. A new hide of a pad whose showing again is out waits until the service has really shown it. With it: 4M-2, and 4M-3's two.
2. **The hiding service.** udevd's ordering read in source and shown in the VM; out with the turns, the wait that goes on after a wait is given up, and the sweep's threads. With it: 4H-1, 4H-3, and the claims in the docs.
3. **The device crate.** 4D-1, 4D-2, 4D-4.
4. **The wire,** once 1 and 2 are merged: `cancel` and the counts out of the wire, the service, the client and the stand-in service.
5. **A review** of all of it, as a whole and not as a diff, the diff being mostly removal.

## State

Steps 1 to 3 are merged, and step 4, the wire, is done on its branch; step 5, the review, follows it.

**udev's order, read and shown.** In systemd 262, the flake's, `event_queue_start()` in `src/udev/udev-manager.c` skips an event that `event_find_blocker()` finds a blocker for ("do not start event if parent or child event is still running or queued"), and the blocker is any earlier event not yet processed with the same id or a devpath that is the same, a parent or a child (`devpath_conflict()`: "true when two paths are equivalent, or one is a child of another"). A worker sends its event to udev's listeners (`device_monitor_send()`, the answer the service waits for) after it has run the rules and their `RUN`s, and only then tells the manager it is done (`PROCESSED=1`). So a request's answer comes after every earlier request of the device has run, and the nodes an earlier re-bind made, the device's children queued after the later request, are finished only once it has run. The hider check writes a re-bind request for a pad, as the service's user, and then has the service hide the pad, with udev held on an earlier event of its HID device; the kernel and udev, read on one socket, go: both requests, then the first's unbind, bind and answer, then the second's unbind, bind and answer. It passed on the code with the turns, before they were taken out, and passes after.

**Step 2, the service: done.** Out go the turns, the wait that went on after a wait was given up, and the sweep's threads; the sweep writes its requests and waits for none, sharing the reconcile's code, and holds the service's state as it does (see Deviations). The comment arguing for state alone after an overrun now rests on udev's order; what it leaves is inputs the kernel removed and made again in the moment between the service reading them and writing its request. 4H-1: a pad's USB interface is its HID device's own parent, where that is one, so a pad on Bluetooth through HIDP gets no `raw-` marker for the adapter's interface, and the rule's `raw-` line is kept to HID devices on the USB bus; its raw node is hidden across a reconnect by `uniq-`, its address being its HID device's from its making; a unit test models HIDP's layout. 4H-3: the sweep leaves a device with a keyboard or a pointer among its inputs as it is, and logs it; the hider check makes such a device through uhid with a hidden pad's address, and its keyboard's input is untouched when the pad is shown again (made anew, input101 to input104, without the fix). 4H-4: the claims that `raw-` is written only on USB and that a re-bind never cuts a keyboard off now hold, and the unit's and the architecture's word on descriptors is one for each pad shown again as a connection closes, none for the sweep; the USB device number that wraps is about the enumeration stamp, which the turns never touched, and is left. `cancel` and the counts are untouched on this branch, for step 4.

**Step 4, the wire: done.** Out go the `cancel` request, the `cancelled` refusal, the count of re-binds in a reply's flags and the mark in the reply to `version`; in the service, the look at the client's next message during a hide's wait, the hide a `cancel` would undo, and the counting of re-binds; in the client, `cancel`, `cancelled`, `release` and the counts its replies carried, `hid` now reading a refusal as the client's other replies do; and the stand-in's `cancel`. Their opcode and status were each the last, so the wire has no gap: six requests, opcodes 1 to 6, five bytes each; replies of six bytes, status 0 or a refusal 1 to 8, `dislodged` the flags' only bit ([the hiding service's plan](hider-rootless.md#a-hide-given-up) has it whole). A hide's wait ends, as before `cancel`, on the client hanging up, whole or its sending alone, and on nothing it sends, which stays queued, an empty message read in its turn as the connection's end. The daemon kept `skip`, for a reply left unread on a pad's connection, and `wait`, `send_hide`, `hid`, `stop_sending` and `version`. Nothing depends on a count any more: the two smaller items of 4H-4 that were about counts, a re-bind the sweep asked just before a hide counted by no reply and a hide given up keeping its count where the rule then didn't act, have nothing left to be wrong about. The hider check's subtests of `cancel` give way to two of a hide given up by hanging up: one closed while its re-bind is held at the node it makes, the pad shown again by a second re-bind and open to the seat user; and one that stops sending while udev is held before its re-bind, the pad shown again once udev runs both, and a hide right after done on its own re-bind with a descriptor that reads.

## Deviations

- **The hiding service's client gained `stop_sending`** in step 1, a half-close beside `hang_up`, where the plan has the client change only in step 4. A pad is busy until the service has really shown it again, and with the connection hung up whole the daemon can't see when that is: stopping sending alone is a hang-up to the service, which closes its end once it has shown the pad, and the daemon reads until then. The wire doesn't change.
- **A hidden pad not kept is given up by hanging up, not shown by a reading:** one whose new node couldn't be read, one whose taking panicked before its name was kept (the fix of 4M-2, the name held by a guard that gives the pad up as it is dropped, unwinding included), and the name a pad hid under before it came back and was hidden afresh. None has a writer to wait for, and the service shows each again as the connection closes; the pad is busy until it has. So no `Show` is carried back for them, and the reactor isn't involved.
- **The release is asked on the first connection, and on the first after the service was found gone,** not on every connection: it was asked on each for the counts. A service upgraded while the daemon held no connection to it is published with the release it had until a connection breaks.
- **A uinput pad has no anchor,** rather than its input device as one: its event node and input device go together, and nothing re-binds it, so its node going is its unplugging, as for a node whose anchor couldn't be read.
- **A connection hung up to show a pad is read to its end by the pad's next hide,** within that hide's errand, and kept until then or until the daemon stops: one per pad at most, as the next hide of the pad reads it, and none on the service's side once it has closed its end.
- **The running service's sweep holds the service's state as it writes its requests,** which the reconcile at start has no need to. The sweep's threads looked again, once their turn came, whether the device was still hidden and named by nothing, so as not to cut off a hide of it asked meanwhile; with the state held, such a hide is either in it, and the device left alone, or writes its request after the sweep's, which udev runs first. Reading sysfs and writing a `uevent` file wait on no client and on no udev event.
