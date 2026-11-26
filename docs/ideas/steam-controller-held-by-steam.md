# Finding a Steam Controller that Steam already holds

Steam opens the raw node of every Valve pad it sees, and while anything holds a Steam Controller's (2026) raw node, the kernel's driver takes the pad's gamepad away. The daemon finds pads by their joystick nodes, so a Steam Controller plugged in while Steam runs is never listed, and a profile that names it waits for it for as long as Steam has it. The pad's HID device, which stays, is all there is to find it by.

**What it would take.** The daemon knowing a pad by its HID device as well as its joystick node: the hotplug watch hearing `hid-steam`'s devices come and go, a pad listed from its HID device's properties (its serial is the input devices' `uniq`, which isn't there while Steam holds it, so it would be read another way), and a setup opening its raw node beside Steam's. Steam would go on reading the pad beside the virtual one, as it does now for a pad the daemon holds ([concern](../concerns/steam-controller-grabbed-not-hidden.md)).

**Why deferred.** It rests on guesses about the real pad and the Puck that nobody can check yet, and waits, with the Puck and Bluetooth, for a pad to check it against. Until then the user guide says to start the profile before Steam, or to close Steam.

**Would bring it in.** A real pad on the desk, and the hiding question decided: if the pad is to be hidden from Steam, finding one Steam holds is part of that.
