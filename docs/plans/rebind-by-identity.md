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

## What is lost and what is not known

- **A re-bind no driver comes back from** leaves the pad away, and so published as connected, until its anchor goes. The count had the same gap, recorded as accepted.
- **A pad unplugged in the moment of a re-bind:** its anchor's removal says so, where the count could leave the pad published as there until plugged in again. A gain.
- **Bluetooth.** A pad BlueZ presents through uhid, or through the kernel's HIDP, keeps its HID device through a re-bind as far as the driver model says: unbinding a driver doesn't remove the device. The hiding service leans on the same, and its VM check runs it for USB and uhid. Not seen on a real pad over Bluetooth; among the hardware checks.
- **Two pads that share a row of the book** (no address, serial or port to tell them apart) were miscounted before; by anchor each node has its own, which should tell them apart. To be confirmed as it is built.

## Steps

Each is a branch of its own, verified by the suites that make no pads, by the whole suite while Steam is closed, and by the VM checks.

1. **The daemon.** The anchor, as a function in the device crate that reads it for a node as the service does; the hotplug watch hearing a HID device's and a USB interface's removal as well as input nodes'; the book by anchor; out with the count, the expectations, the guard and its flush, the settling of hides given up, and the daemon's use of `cancel` and of counts; showing a pad again still waits for its writer and then asks the restore and reads its reply. A new hide of a pad whose showing again is out waits until the service has really shown it. With it: 4M-2, and 4M-3's two.
2. **The hiding service.** udevd's ordering read in source and shown in the VM; out with the turns, the wait that goes on after a wait is given up, and the sweep's threads. With it: 4H-1, 4H-3, and the claims in the docs.
3. **The device crate.** 4D-1, 4D-2, 4D-4.
4. **The wire,** once 1 and 2 are merged: `cancel` and the counts out of the wire, the service, the client and the stand-in service.
5. **A review** of all of it, as a whole and not as a diff, the diff being mostly removal.

## State

Decided; steps 1 to 3 under way.

**udev's order, read and shown.** In systemd 262, the flake's, `event_queue_start()` in `src/udev/udev-manager.c` skips an event that `event_find_blocker()` finds a blocker for ("do not start event if parent or child event is still running or queued"), and the blocker is any earlier event not yet processed with the same id or a devpath that is the same, a parent or a child (`devpath_conflict()`: "true when two paths are equivalent, or one is a child of another"). A worker sends its event to udev's listeners (`device_monitor_send()`, the answer the service waits for) after it has run the rules and their `RUN`s, and only then tells the manager it is done (`PROCESSED=1`). So a request's answer comes after every earlier request of the device has run, and the nodes an earlier re-bind made, the device's children queued after the later request, are finished only once it has run. The hider check writes a re-bind request for a pad, as the service's user, and then has the service hide the pad, with udev held on an earlier event of its HID device; the kernel and udev, read on one socket, go: both requests, then the first's unbind, bind and answer, then the second's unbind, bind and answer. It passed on the code with the turns, before they were taken out, and passes after.

**Step 2, the service: done.** Out go the turns, the wait that went on after a wait was given up, and the sweep's threads; the sweep writes its requests and waits for none, sharing the reconcile's code, and holds the service's state as it does (see Deviations). The comment arguing for state alone after an overrun now rests on udev's order; what it leaves is inputs the kernel removed and made again in the moment between the service reading them and writing its request. 4H-1: a pad's USB interface is its HID device's own parent, where that is one, so a pad on Bluetooth through HIDP gets no `raw-` marker for the adapter's interface, and the rule's `raw-` line is kept to HID devices on the USB bus; its raw node is hidden across a reconnect by `uniq-`, its address being its HID device's from its making; a unit test models HIDP's layout. 4H-3: the sweep leaves a device with a keyboard or a pointer among its inputs as it is, and logs it; the hider check makes such a device through uhid with a hidden pad's address, and its keyboard's input is untouched when the pad is shown again (made anew, input101 to input104, without the fix). 4H-4: the claims that `raw-` is written only on USB and that a re-bind never cuts a keyboard off now hold, and the unit's and the architecture's word on descriptors is one for each pad shown again as a connection closes, none for the sweep; the USB device number that wraps is about the enumeration stamp, which the turns never touched, and is left. `cancel` and the counts are untouched on this branch, for step 4.

## Deviations

- **The running service's sweep holds the service's state as it writes its requests,** which the reconcile at start has no need to. The sweep's threads looked again, once their turn came, whether the device was still hidden and named by nothing, so as not to cut off a hide of it asked meanwhile; with the state held, such a hide is either in it, and the device left alone, or writes its request after the sweep's, which udev runs first. Reading sysfs and writing a `uevent` file wait on no client and on no udev event.
