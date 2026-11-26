# The Steam Controller

## State

- **Status:** drafted with the maintainer on 2026-11-25, who then stepped away for the night with the coordinator left to build through the stops. Research is under way.
- **Next action:** the research below, then the design it leaves to be decided, recorded under For review; then the build steps.
- **Branch:** `feat/steam-controller`, from `docs/user-guide` at `383c798`, so the new user guide is there for the agents to read. Nothing is pushed until the maintainer has looked. When the guide's pull request (#63) has merged, this branch's own goes to `main`.

## Goal

CtrlAssist merges controllers with buttons, sticks, triggers, a touchpad and motion sensors: a DualSense, a DualShock 4. Valve's new Steam Controller has all of those and more: two trackpads, four back buttons, capacitive grips and sticks, and haptics beyond plain rumble. This plan makes it a controller CtrlAssist reads, as a primary or an assist, and finds out whether CtrlAssist can also present one to games.

The maintainer has no Steam Controller yet. The kernel does: `hid-steam` in Linux 7.3.4, which the development host runs, claims four Valve product numbers beyond the 2015 controller's and the Deck's, by the names Ibex (`28de:1302` on USB, `28de:1303` over Bluetooth), Proteus (`28de:1304`) and Nereid (`28de:1305`). Which name is which piece of hardware is for the research to say.

## Decided

By the maintainer, 2026-11-25:

- **As an input first.** A real Steam Controller plays a part in a profile: its standard controls are read and merged and rumble reaches it.
- **As an identity too, if a spike says it can be.** A virtual Steam Controller is built only once a spike has shown that the kernel's driver and SDL, and Steam if it can be tried, take a stand-in for the real thing. It is the profile's choice and is not tied to what is behind it: it carries whatever the controllers behind it have, as the DualSense identity does, and what only Valve's controller understands passes through only to a real one.
- **Everything it has is in scope:** the standard controls and rumble, both trackpads, the back buttons, capacitive touch and its richer haptics, and whatever else the research finds that was forgotten.
- **Three transports:** a USB cable, the Puck, and Bluetooth. The 2015 controller and the Deck's built-in controls are not: what comes of them for nothing is kept, and the rest is filed.
- **Behind a virtual controller that isn't a Steam Controller, what fits is mapped and the rest dropped,** each drop said in the guide: the right trackpad is the virtual DualSense's touchpad, motion goes as it does today, and a back button can be a profile's own button (the Gyro button, a toggle's switch) but isn't shown to games.
- **Built against a fake.** A fake Steam Controller is made through uhid for the kernel's own driver to bind, as the fake DualSense is, from the driver's source. What only hardware can show is filed as a concern, to close when the maintainer's arrives.
- **Overnight, through the stops.** The coordinator runs Opus agents, each build step in a worktree of its own and read by a second agent that hasn't seen the first's reasoning, and merges once the gates pass. A spike's finding and each decision made without the maintainer are recorded under For review and not waited on. A stop is kept only where a rule says to ask, as for a new timeout, or where something is blocked.

## Guardrails

- No agent touches the maintainer's real controllers, the installed hiding service or its socket, or `/etc/nixos`.
- The whole test suite, and anything that makes fake pads, runs only while Steam is closed; an agent looks first (`pgrep -x steam`).
- No push, and no change outside this repository's worktrees.
- A web search carries nothing of the maintainer's or of this machine: a product's name, a driver's name, an error's text.

## To find out

Research, by agents in parallel, each reporting what it read and where, and what it could not settle.

1. **The kernel's driver** (`drivers/hid/hid-steam.c` at 7.3.4). Which product number is which hardware. What it presents: its input devices, their buttons and axes, the trackpads, the sensors device, the back buttons, capacitive touch. Its reports, by transport: USB, the Puck with its slots and wireless state, Bluetooth. Lizard mode, where the controller types and points with no client, and what turns it off. What it does when a program opens the raw node, as Steam does. Rumble and haptics. The serial number and the battery. The first kernel release that has each of this. And what is newer than 7.3.4: the driver on the kernel's master branch and in the HID tree's next, and patches for it still on the linux-input list, since the support is recent and may still be moving (the maintainer's pointer: Phoronix's note on HID in Linux 7.3, `https://www.phoronix.com/news/Linux-7.3-HID`).
2. **SDL and Steam.** SDL's driver for the new controller, what it reads and writes, and what it and Steam need of a device before they take it for one. Steam's own udev rules for the raw node. What is published of the real controller's report descriptor and feature replies.
3. **CtrlAssist as it is.** What the pad state, the kernel layout, the motion reader, the mux's rules, the virtual DualSense and the hiding rule would each need for a controller with two trackpads, back buttons, and keyboard and mouse interfaces beside its gamepad. Where the hiding service refuses it today, and why.

## Research: the kernel's driver

Read by an agent on 2026-11-26 from `drivers/hid/hid-steam.c` at Linux 7.3.4, with line numbers of that file; what is marked an inference was not read from source.

**Which is which** (the device table, 2751-2770): `28de:1302` is the controller on a USB cable; `1303` the controller over Bluetooth LE; `1304` the Puck; `1305` the receiver inside a Steam Machine. SDL calls the controller Triton.

**First release.** Linux 7.3 (tagged 2026-10-18), nearly all of it in one commit, `0a80b4e8ec6a`. 7.2 has none of it. Nothing newer was found on 2026-11-26: the kernel's master, linux-next and the HID tree differ from 7.3.4 by one unrelated line, and patchwork's linux-input has no later patch for the driver (lore.kernel.org and Phoronix refused the agent's fetches).

**What the driver presents.** Two input devices and a battery, each with the controller's serial as `uniq`:

- A gamepad: the face buttons, D-pad (as buttons), bumpers, the triggers pressed home, both stick clicks, Select, Start, Mode, the quick-access button (`BTN_BASE`), both trackpads' clicks (`BTN_THUMB` left, `BTN_THUMB2` right), and the four back buttons (`BTN_GRIPL`, `BTN_GRIPL2`, `BTN_GRIPR`, `BTN_GRIPR2`). Sticks on `ABS_X/Y` and `ABS_RX/RY`; the left trackpad on `ABS_HAT0X/Y` and the right on `ABS_HAT1X/Y`, each -32767 to 32767 and reported as 0 while untouched; the analog triggers on `ABS_HAT2Y` (left) and `ABS_HAT2X` (right), 0 to 32767.
- "Steam Controller Motion Sensors": accelerometer on `ABS_X/Y/Z` at 16384 to the g, gyroscope on `ABS_RX/RY/RZ` at 16 to the degree a second, and the controller's own clock as `MSC_TIMESTAMP`.
- Not presented at all: whether a trackpad is touched, as an event; a trackpad's pressure; capacitive touch on the sticks and the grips; the controller's own orientation; and every haptic command beyond rumble.

**The report** is the same on every transport: input `0x42` (54 bytes) or `0x45` (46 bytes, without the orientation), battery `0x43`, a wireless event `0x79`, rumble as output `0x80`, a pulse as output `0x81`, and commands as feature reports 1 (controller) and 2 (receiver), 64 bytes each. Its layout is in the file at 2324-2391 and agrees with SDL's.

**Five things that weren't expected:**

1. **In lizard mode the gamepad says nothing.** With the driver's `lizard_mode` parameter at its default, the controller's input devices exist and are silent until the player holds the menu button for about half a second, which switches the driver to its gamepad mode (2447-2458). Opening the event node doesn't switch it, as it does for the 2015 controller. Writing 0 to `/sys/module/hid_steam/parameters/lizard_mode` makes events flow always.
2. **Opening the raw node takes the input devices away.** The raw node belongs to a client device the driver makes; while anything has it open, the driver removes the gamepad, the motion device and the battery, and sends nothing itself. They are made anew, with new node numbers, when the last opener closes it. Steam opens it, and so does SDL's own driver by default, so a game through SDL may do so as well (an inference).
3. **Behind a Puck a controller carries the Puck's product number** and the name "Wireless Steam Controller": only its serial tells it from another, or from the same controller on a cable.
4. **Rumble needs `CONFIG_STEAM_FF`,** which the development host's kernel is built without, so there the gamepad has no force feedback at all.
5. **SDL reads a newer report, `0x47`,** that the kernel ignores. If a controller's firmware sends only that, the kernel's input devices would say nothing (an inference).

**The Puck** is one USB device of seven interfaces: two that aren't HID, four slots, and one for the charging pins. Each slot is a HID device of its own whose raw node is always there; its gamepad, motion and battery devices come when a controller joins the slot (wireless event 2, or any input report) and go when it leaves (event 1).

**For a fake through uhid.** The descriptor must declare input reports `0x42`, `0x43`, `0x45` and `0x79` and feature report 1 (and 2 for a receiver) at 64 bytes or more. No reply is needed for the driver to bind, but each command left unanswered costs five seconds, some twenty in all, so the fake answers: the serial (`AE`), the attributes (`83`), and the settings writes. The driver makes no USB call, so a uhid device binds as a real one does (an inference from what it includes).

**Not settled without hardware:** the real report descriptors, and so where lizard mode's keyboard and mouse are and whether they share the gamepad's HID device; whether current firmware sends `0x47`; whether a cabled controller reports motion without being asked; the triggers' real range and the trackpads' vertical sense; whether the controller falls back to lizard mode by itself.

## Steps

A first cut, to be redrawn once the research is in.

1. **A fake Steam Controller** in the test crate, through uhid, that `hid-steam` binds; the VM checks load the driver.
2. **Read as a pad:** its kind and name, what tells one from another, its standard controls through the kernel's layout, and rumble.
3. **Motion and the right trackpad,** by whichever road the research finds: the driver's own input devices, or its raw report as a DualSense's is read.
4. **The pad state grows:** a second trackpad, back buttons, capacitive touch; the mux's rules for each; what a virtual DualSense, an Xbox pad and a generic one each do with them.
5. **Hiding:** a controller that also types and points, and that Steam opens by its raw node.
6. **The Puck and Bluetooth.**
7. **The window, the tray and the guide:** its badge, what the Pads page says of it, the guide's pages and Requirements' kernel.
8. **Spike: a virtual Steam Controller.** Does the kernel's driver bind one made through uhid, and do SDL and Steam take it? Its finding decides step 9.
9. **The Steam Controller identity,** if the spike says so.
10. **Concerns for the hardware,** one per thing only a real controller can show, and the plan retired.

## For review

Decisions made and findings written while the maintainer was away, each for them to read.

None yet.

## Deviations

None yet.
