# The Steam Controller

## State

- **Status:** drafted with the maintainer on 2026-11-25, who then stepped away for the night with the coordinator left to build through the stops. The research is in, and the design it left to decide is under For review. Building.
- **Next action:** steps 2 and 3. Step 1's spike is done: its finding is under "Spike: the kernel's driver and a uhid stand-in" and For review 8, and the steps from 4 on rest on it. Step 4 is done: `FakeSteamController` in the test crate, its tests in the device crate's, and the `device` check on Linux 7.3 (For review 9).
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

## Research: SDL, Steam and what is published

Read by an agent on 2026-11-26 from SDL at 3.4.18 and at its main branch (`3ebae2a7`), Steam's udev rules, and other projects' sources and captures.

- **SDL reads the raw node, never the event nodes,** with one driver for every transport, `SDL_hidapi_steam_triton.c`. It asks the controller nothing before taking it: it only writes. Every three seconds it sets lizard mode off, and on its main branch the motion sensors on, as a feature report (`01 87 03 09 00 00`, with `30 18 00` for the sensors), because the controller falls back to lizard mode by a watchdog of its own, and because Steam turns the sensors off when it has no use for them. Rumble is output report `0x80`, sent again every 40 ms while it is nonzero, the controller stopping by itself some 50 ms after the last.
- **The report as SDL reads it** agrees with the kernel's. Of what the kernel leaves out: a trackpad is touched while its bit is set (left `0x2000000`, right `0x200000` of the 32-bit buttons); the sticks' capacitive touch is `0x1000000` (left) and `0x100000` (right), the grips' `0x20000000` (left) and `0x10000000` (right); a trackpad's pressure is the 16 bits after its position. The gyroscope's full scale is 2000 degrees a second and the accelerometer's 2 g; SDL's axes are X, Z and minus Y of the report's. The back buttons are, in SDL's order of paddles, R4, L4, R5, L5. SDL's main branch also reads a report `0x47`, with shorter timestamps, which the kernel doesn't.
- **Steam opens the raw node itself** and presents what it reads to games as a virtual pad of its own, through uinput. Its udev rules give the seat every raw node of vendor `28de`, a uhid device's among them (`KERNELS=="000[356]:28DE:*"`).
- **The real controller's report descriptor is published:** 372 bytes, captured from a retail controller on a cable, in `Nonary/libvirtualgamepad` (MIT), with Steam's whole exchange with it. It is one HID interface of three collections: a mouse (report `0x40`), a keyboard (`0x41`), and Valve's own, which has the input reports above, the output reports `0x80` to `0x89` and the two feature reports. So lizard mode's keyboard and mouse are the same HID device as the gamepad.
- **A stand-in has already been taken for one.** Two projects present a virtual `28de:1302` that Steam lists and uses, one of them through Linux uhid, bound by `hid-steam` on 7.3 and read by SDL (`anderstvoss/virtualgamepad`). Steam listed it once the fake answered its attributes (`83`) with a real controller's, its serials (`AE`), its settings (`87`) and a few acknowledgments. No signed challenge appears in either project's captures. Steam offers a firmware update to a controller whose firmware it finds old, and what it accepts it takes over, into Steam Input, as it does a real one. A Puck can't be stood in for through uhid for SDL, which wants the USB interface numbers a uhid device hasn't.
- **Over Bluetooth a real controller is itself a uhid device,** made by BlueZ, so a fake on that bus is on the real one's road.
- **Not published:** the report descriptors of the controller over Bluetooth and of a real Puck's slots (a clone's is, 15 bytes longer for report `0x47`); the watchdog's length; what several reports and commands mean.

## Research: CtrlAssist as it is

Read by an agent on 2026-11-26, with file and line for each claim in its report; the most that bears on the design:

- **The pad state has room** for the back buttons and the capacitive touches as buttons (64 buttons, 16 axes, and the controller database already has places 16 to 19 for four paddles). A second trackpad is a change of type: there is one `Touchpad`, with its mux rule and state, and every user of it is touched.
- **The kernel layout would misread `hid-steam`'s gamepad:** its left trackpad is on the axes every other pad has its D-pad on, its analog triggers on axes the layout takes for spares, and its extra keys are handed places in order with no skip of the touchpad click's.
- **Nothing reads a second event node of a pad,** and the hiding service hands out a joystick node and, for the `playstation` driver alone, a raw node.
- **A pad is found, named and hidden by its joystick node.** The hiding service refuses a pad whose HID device also has a keyboard or a pointer, which this controller's has, so today it would only be grabbed. Its markers take `uniq` for an address, and `hid-steam`'s is a serial. A Puck is one USB device with several controllers, which one re-bind would cut off together.
- **`model:` names a product number,** and this controller has three, one a transport.

## Spike: the kernel's driver and a uhid stand-in

**Question.** Does `hid-steam` do with a stand-in for the 2026 controller what the research read in its source: bind it, keep its gamepad silent in lizard mode, give way to whoever opens the raw node, and pass reports both ways through that node? And what do udev and the hiding service's checks make of its nodes?

**Method.** `spikes/steam`, a throwaway workspace and NixOS check, run by an agent on 2026-11-26 with `nix build --impure --expr 'import ./spikes/steam/test.nix { }' -L`: a virtual machine on nixpkgs' `linuxPackages_7_3` (7.3.4, from the binary cache) with `uhid` and `hid-steam` loaded at boot and the hiding service's udev rule installed. A stand-in made through `/dev/uhid`, USB bus, `28de:1302`, answering the driver's feature requests (the serial `AE`, the attributes `83` with a retail controller's values, and an acknowledgment of anything else), in three descriptors one after another: one written for the spike with Valve's collection alone (input `0x42` of 54 bytes with its number, `0x45` of 46, `0x43` of 15, `0x79` of 2; output `0x80`, `0x81`; feature 1 and 2 of 64); the same with `0x47` declared; and the published retail descriptor, with its mouse (`0x40`) and keyboard (`0x41`) collections. Every uhid event, every input event, every kernel uevent and every read of the raw node was logged with a monotonic time; the logs and a `finding.json` are in the check's result. What is marked an inference was not seen.

**Finding: the driver does what the research read,** with these details and these corrections:

- **It binds at once.** The gamepad's input device came 2 to 10 ms after the device was created, and nothing waited on a timeout. Probe asked the serial (`AE 16 01`, then a GET), the attributes (`83`, then a GET), and set lizard mode on (`85`, `8E`); dmesg says `Steam Controller 'SPIKEUNIT0001' connected`, the serial being what the stand-in answered.
- **What appears:** under the HID device (`/sys/devices/virtual/misc/uhid/0003:28DE:1302.NNNN`, driver `hid-steam`) the gamepad "Steam Controller" with `eventN` and `jsN`, the motion device "Steam Controller Motion Sensors" with `eventN` and also a `jsN`, and a battery `power_supply` named `steam-<serial>` that brings a `thermal_zone` and an `hwmon` with it. Both input devices have the HID device's `phys`, the controller's serial as `uniq`, and bus, vendor, product and version `0003:28de:1302:0307`. The gamepad's keys are the 24 the research listed and no `EV_FF`; its axes are as listed, the sticks and trackpads ±32767 (the trackpads with fuzz 256), the triggers on `ABS_HAT2Y`/`ABS_HAT2X` 0 to 32767. The motion device has `INPUT_PROP_ACCELEROMETER`, `MSC_TIMESTAMP`, and six axes of ±32768 at 16384 to the g and 16 to the degree a second.
- **udev:** the gamepad's nodes are `ID_INPUT_JOYSTICK=1` and nothing else of `ID_INPUT_*` (no `ID_INPUT_KEY`), tagged `uaccess`; the motion device's are `ID_INPUT_ACCELEROMETER=1` only, not tagged, but **joydev gives it a `js` node all the same**, which a program that lists `/dev/input/js*` sees as a second joystick. The HID device's `HID_UNIQ` is the uhid device's own string (a real one's is its USB serial number, an inference).
- **The raw node is the client device's, a second HID device:** a sibling of the real one under the same parent (`0003:28DE:1302.NNNN+1`, driver `hid-steam`, `MODALIAS` group `0103`), with the same `HID_ID`, `HID_NAME` and `HID_PHYS` but an empty `HID_UNIQ`. The real HID device has no raw node at all. On a stock machine the node is `root` 0600 and not tagged `uaccess`: a seat user can't open it without a rule of Steam's or CtrlAssist's.
- **Lizard mode, as read.** At the parameter's default, button and stick reports made no event on either input device, though both were open. The menu bit held for 451 ms switched to gamepad mode: the stand-in was sent `81` (clear the mappings), `87 06 09 00 00 47 00 00` (lizard mode off, Steam's watchdog off) and two haptic pulses as output reports `0x81`; events flowed from then, `BTN_START` among them. **It is a toggle:** holding the menu button again went back to lizard mode (`85`, `8E`, two pulses, every control zeroed), silent again. Writing 0 to the module parameter sent `81` and `87 …` at once and events flowed at once; writing 1 sent `85` and `8E`.
- **What the gamepad makes of a report:** the left stick's Y is inverted (the report's −20000 is `ABS_Y` 20000), the right's too; a trackpad's place is reported only while its touch bit is set, and 0 otherwise; the triggers' values pass as they are.
- **The battery follows report `0x43`** at lizard mode's default too (capacity 77, discharging, 3600 mV as sent).
- **Opening the raw node takes away the gamepad, the motion device and the battery** (with its thermal zone and hwmon), within 2 to 5 ms, and the driver sends the controller nothing. Readers of the event nodes get `ENODEV`. Held menu does nothing while it is open. **The mouse and keyboard of the retail descriptor stay,** and keep typing and pointing.
- **Closing it brings them back** within 9 to 15 ms, under new input device numbers; the event and `js` nodes took **the lowest free numbers, which here were the same as before** (the research's "new node numbers" holds for `inputN`, not necessarily for `eventN`). The driver asks the serial and the attributes again and sets lizard mode as its own state says (`85`, `8E` here).
- **Through the raw node, reports pass byte for byte, but only declared ones of their full length.** `0x42`, `0x45`, `0x43`, `0x79`, and the retail descriptor's `0x44`, which the driver ignores, were read exactly as sent. **A report whose number the descriptor doesn't declare is dropped by the HID core before the driver or the raw node sees it:** `0x47` never reached the raw node under the spike's descriptor or the retail one, and reached it byte for byte once declared (it made no input event either way). **A report shorter than its declared size is dropped whole** on 7.3 (a 30-byte `0x42`, a 40-byte `0x47`), not padded.
- **Writes reach the controller as they are:** output report `0x80` written to the raw node arrived as uhid's `OUTPUT` with the output type and the same 10 bytes; `HIDIOCSFEATURE` of `01 87 03 09 00 00` padded to 64 arrived as `SET_REPORT` of feature report 1, 64 bytes; `HIDIOCGFEATURE` arrived as `GET_REPORT` and returned the reply.
- **The retail descriptor's mouse and keyboard are one input device,** named after the HID device (for a real one its USB product string, an inference), with an `event` and a `mouse` node, 107 keys from `KEY_ESC`, `REL_X/Y` and both wheels, and `ID_INPUT_KEY`, `ID_INPUT_KEYBOARD` and `ID_INPUT_MOUSE`; dmesg calls the HID device a "Mouse". `0x41` with Enter gave `KEY_ENTER`, `0x40` gave `REL_X`/`REL_Y`.
- **`CONFIG_STEAM_FF` is not set** in nixpkgs' 7.3.4 kernel, as on the development host; no `FF_RUMBLE`.

**What the hiding service would make of it,** from these properties by `pad.rs`'s checks:

- **The spike's descriptor:** the gamepad passes `is_only_a_pad`, nothing under its HID device is a keyboard or a pointer, so `identify` would take it; markers `hid-<the real HID device>` and `uniq-0003:000028DE:00001302-<HID_UNIQ>` (on a real cable also `serial-28de-1302-<USB serial>`, an inference). The motion device is one of the pad's other nodes.
- **The retail descriptor:** the gamepad still passes `is_only_a_pad`, but `shares_with_keyboard_or_pointer` finds the mouse-and-keyboard device, so `identify` refuses it `Unsupported`, as the research read. The rule can't see that sharing: it gave the HID device's `uevent` file to the service's group in both cases.
- **The raw node** is refused by `raw_node` (`hid-steam` isn't in `RAW_DRIVERS`), and even with the driver listed it would not be found there: it isn't under the pad's HID device but the client's. No marker the service makes now names it (`hid-` names the other device, and `uniq-` needs a `HID_UNIQ` the client hasn't).

**So.** The design under For review stands; what it rests on is now seen, not read. What the steps take from it:

- **The fake (step 4)** answers `AE` and `83` as a GET after the SET, acknowledges the rest, declares every report it sends at the length it sends it, and expects a toggle on the menu button. Its raw node is the client device's.
- **Reading the raw node (step 5)** needs a rule that gives the seat that node, a sibling HID device found from the pad's by parent and `HID_PHYS`; a report the firmware sends undeclared, or short, never arrives, so `0x47` is read only where a descriptor declares it. While CtrlAssist holds the node the driver sends nothing, so lizard mode off is CtrlAssist's to keep.
- **Holding a pad through its joystick's going (step 5) and finding one Steam holds (step 7)** go by the real HID device, which stays while the raw node is held; its `inputN` changes, and its event node may come back under the same name.
- **Hiding (For review 4)** is refused for the retail descriptor exactly as read; a raw-node marker would have to name the client device.

## Steps

Redrawn on 2026-11-26 from the research. Each is an agent's in a worktree of its own, read by another, and merged by the coordinator once the gates pass.

1. **Spike: the kernel's driver and a uhid stand-in,** in a virtual machine with a 7.3 kernel, where a made-up controller troubles nobody's desktop or Steam. Does `hid-steam` bind it, and how long does it take; which devices come, with which names and properties; does the gamepad say anything before the menu button is held; what happens to the devices as the raw node is opened and closed; do an output report and a feature report written to the raw node reach the fake; what udev and the hiding service's tests make of its nodes. Its finding goes under For review, and steps 4 on rest on it.
2. **The pad state grows:** a second trackpad, with the mux's rule and state for it and the demux's passing of it; places and names for the four paddles and the four capacitive touches; `paddle-1` to `paddle-4` as buttons a profile can name.
3. **The report, read and written,** as plain code with no device: the input reports to a sample (buttons, sticks, triggers, both trackpads with touch and pressure, the capacitive bits, motion in the units the pad state uses, the battery, a slot's joining and leaving), and the rumble report and the settings frames composed.
4. **A fake Steam Controller** in the test crate, through uhid, answering what the driver asks, and the VM checks on a kernel that knows it.
5. **Read in a setup:** the pad's kind, its raw node opened and read on the input thread for every control, rumble written, lizard mode kept off, and the pad held through its joystick's going.
6. **Behind each identity:** the first trackpad to a virtual DualSense's touchpad, motion as today, the paddles as a profile's buttons; what is dropped.
7. **Found while Steam holds it,** the Puck's slots, and Bluetooth: a pad known by its HID device.
8. **The window, the tray and the guide.**
9. **The Steam Controller identity,** if the maintainer wants what For review 7 says follows.
10. **Concerns for the hardware,** and the plan retired.

## For review

Decisions made and findings written while the maintainer was away, each for them to read.

**1. The controller is read from its raw report, not from the kernel's input devices** (2026-11-26, the coordinator's, on the three researches). `hid-steam`'s gamepad is silent until the player holds a button or a module parameter is changed for every Steam controller on the machine; it never says whether a trackpad is touched, nor anything of the capacitive sensors; it has no rumble on a kernel built without `CONFIG_STEAM_FF`, the development host's among them; and it is removed whenever anything opens the raw node, as Steam and SDL do. The raw report has all of it, the same on every transport, and is what SDL reads. So a Steam Controller in a setup is read as a DualSense's motion is, from its raw node, but for every control; rumble is the output report; and while a setup holds it CtrlAssist keeps lizard mode off as SDL does.

**2. What that costs, and is not built yet.** Opening the raw node makes the kernel remove the controller's joystick node, which is what CtrlAssist finds, names and hides a pad by. The controller is still found by that node while nothing holds it. Holding it through its going, and finding one that Steam already holds, need the daemon to know a pad by its HID device as well. That is for after the spike below has shown the kernel doing what the research read.

**3. Two things the controller's protocol asks for at intervals:** rumble again every 40 ms while it is nonzero, and lizard mode off every few seconds. They are the controller's own, as a virtual DualSense's report every 10 ms is a real one's, and they go on the input thread's timer as that does. They are put here because a rule says to ask before a new duration: neither is CtrlAssist's to choose, but the maintainer hasn't been asked.

**4. Hiding is left as it is, and so the controller is not hidden.** The hiding service refuses a device that could type or point, and this controller's one HID device has a keyboard and a mouse. Making an exception for it loosens a rule the service's safety rests on, which is the maintainer's to decide, not the coordinator's. Until then: holding the raw node takes the joystick away from games that read event nodes, and Steam and games through SDL's own driver, which read the raw node, still see the real controller. That is the largest gap this plan leaves, and it is in the guide when the guide is written.

**5. Names and places for what is new.** The back buttons are `paddle-1` to `paddle-4` in SDL's order (upper right, upper left, lower right, lower left), at the controller database's places for paddles. The capacitive touches are buttons in no group, so a resting thumb never owns a stick. The pad state gains a second trackpad: the first is a DualSense's touchpad and a Steam Controller's right, the second a Steam Controller's left, each with the mux's touchpad rule and a state of its own.

**6. The fake's descriptor is written here, not copied.** The published capture of the real descriptor is MIT-licensed and a fact about hardware; the test crate's fake declares what the kernel's driver needs from this plan's own reading. Whether a virtual Steam Controller ships the real descriptor's bytes is for the identity's step and the maintainer.

**7. The identity's spike is half answered already,** by others: a uhid stand-in is bound by the driver, read by SDL and listed by Steam. What is left for it is whether CtrlAssist wants what follows: Steam takes such a controller into Steam Input and shows games a pad of its own, and may offer it a firmware update.

**8. The kernel's driver, seen** (2026-11-26, a spike in a virtual machine; [the finding](#spike-the-kernels-driver-and-a-uhid-stand-in)). It does what the research read, with corrections: holding the menu button toggles lizard mode, not only leaves it; the raw node is a sibling HID device's, root's alone on a stock machine and named by no marker the hiding service makes; a report the descriptor doesn't declare, `0x47` under the retail descriptor among them, or one shorter than declared, reaches neither the driver nor the raw node; the motion device has a `js` node; the battery brings a thermal zone; and an event node may come back under its old name.

**9. The fake, as built** (2026-11-26, the agent building it). Three choices the plan didn't make. The fake with lizard mode's mouse and keyboard, wanted for a test of what the hiding service makes of the real shape, is in the test crate but can be built only with a proof that the process runs in a virtual machine check (a hypervisor, and a file the checks' configuration puts in `/etc`); the `device` check makes one, and the hiding service's test of it is left for that step. A test that makes the fake skips while Steam is running, which would take it for a controller of its own, as it skips on a kernel older than 7.3 even where fake pads are required. And the whole `device` check now runs on `linuxPackages_7_3`, its other subtests with it, rather than a second machine on the newer kernel.

## Deviations

- **The steps were redrawn from the research** (2026-11-26): a spike of the kernel's driver first, the controller read from its raw report and not the driver's input devices, hiding left as it is, and the identity's own spike mostly answered by other projects. For review has each.
