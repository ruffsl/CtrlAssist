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
