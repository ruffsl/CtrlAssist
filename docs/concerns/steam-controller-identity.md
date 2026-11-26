# A Steam Controller is known by its serial, which no pad has shown

A Steam Controller (2026) pad's input devices carry its serial number as `uniq`, which the kernel's `hid-steam` asks of the pad each time it makes them. The daemon takes that for the pad's address and knows it as `address:<serial>`, the same, by the driver's source, on a cable, behind a Puck and over Bluetooth, and as its joystick goes and comes back for each hold of its raw node. A profile names it so; an address of letters and digits is matched in either case.

**What only the pad can say:**

- **That the serial is the same on every transport,** as the driver's source says. If not, a profile that names the pad on a cable doesn't find it behind the Puck.
- **That the serial is always read.** If the driver's question goes unanswered, it names the pad `XXXXXXXXXX`. The question is asked again whenever the pad's input devices are made anew, which a setup's hold and its end each cause; a pad that answered once and not the next time would come back as another pad, and a setup that holds it would wait for it for ever. Two pads that both failed would share an address.
- **That the pad's client HID device is the only raw node in its USB interface's group `0103`,** which is how its raw node is found.
- **How quickly the joystick comes back once the raw node is let go,** which is how long a stopped profile's pad is away.

**Risk.** Low to medium: a pad that came back under another name would leave a setup held, which the window and the tray say, and replugging the pad puts it right.

**Mitigation.** The daemon never takes `XXXXXXXXXX` for anything but an address, so a pad that comes back so is a new pad, not some other pad's. [A pad's identifier across transports](pad-identity-across-transports.md) is the same concern for other pads.

**Closes when** a pad has been seen on each transport. `ctrlassist pads` gives the same `address:` on a cable, behind the Puck and over Bluetooth; `journalctl -k | grep "Steam Controller"` never shows `'XXXXXXXXXX' connected` over twenty starts and stops of a profile of it and twenty replugs; and the pad is listed connected again within a second or two of each stop.
