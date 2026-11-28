# A Steam Controller behind a Puck or over Bluetooth is untried

A Steam Controller (2026) reaches the machine three ways: on a USB cable (`28de:1302`), through its USB receiver, the Puck (`28de:1304`, and `1305` for the receiver inside a Steam Machine), or over Bluetooth LE (`28de:1303`). The fake that every test uses is a cable's, through uhid; a Puck's slot is stood in for by the same fake with the Puck's product number and its wireless events, and Bluetooth by its product number on the Bluetooth bus. Neither the real Puck's slots nor a pad over Bluetooth have been seen, and their report descriptors are not published (a clone Puck's is, 15 bytes longer for report `0x47`).

**What only the hardware can say:**

- **That each transport's raw node is found and taken.** The daemon finds the pad's raw node as the client HID device the driver makes beside the pad's own: the sibling under the same parent, in the driver's group `0103`, that says the same `HID_PHYS`. A Puck is one USB device of several interfaces, each slot a HID device of its own with its raw node always there, and a charging-pin interface the driver starts with the same numbers; a raw node whose descriptor declares neither state report is refused. Whether each real slot's client device is the only one that matches, and declares the state reports, is for the Puck to show.
- **Two pads over Bluetooth on one adapter** say the same `HID_PHYS` under the same parent, so which raw node is whose can't be told, and a start that wants either is refused as ambiguous. Whether real BlueZ devices are laid out so is not known.
- **A pad's product number changes with its transport,** and with it the model a profile's `model:` names: `model:28de:1302` takes the pad on a cable and not over Bluetooth. A pad named by its address, which is its serial, is the same pad on all three ([identity](steam-controller-identity.md)).
- **A slot's pad joining and leaving.** A pad in a slot is gone, while a setup holds it, when the slot's wireless event says it left; the slot's HID device stays. Whether the real Puck sends that event, and whether a pad switched off without it simply stops reporting, which the daemon would never take for the pad gone, is for the Puck to show.
- **Feature reports over the air,** which the kernel's driver retries on `EPIPE` ([lizard mode](steam-controller-lizard-mode.md)).

**Risk.** Medium: the Puck is how the pad ships, so most people will meet it there first. A failure would show as a pad that won't start in a setup, with the reason in the setup's state, rather than as one that misbehaves.

**Mitigation.** The fake covers the slot's events and the Bluetooth product number, and the daemon's tests cover a pad leaving a Puck's slot while a setup holds it. Every failure to find or open a raw node is said in the setup's reason.

**Closes when** a pad has played through a setup behind a Puck and over Bluetooth, and two over Bluetooth at once. For each: `ctrlassist pads` lists it under the same `address:` as on the cable; a profile naming it starts and its presses reach the virtual pad; switching the pad off while the profile runs holds the setup, and switching it on again resumes it. Behind the Puck, also with two pads in two slots. `sudo hid-recorder` on each slot's raw node, with nothing holding it, records the descriptors, to be kept in the test crate's notes.
