# Finding: virtual pad identity

Measured 2026-11-21 on the development host with `spikes/identity/`, a DualSense on USB as the model, SDL2's `sdl2-jstest`, and Steam (client 1794536281, read from its `logs/controller.txt`). No Xbox pad was connected, so the Xbox rows test a virtual pad carrying Xbox IDs, not a comparison with a real one.

## Verdict

**uhid for PlayStation identities, uinput for everything else.**

The plan's rule was that uhid is kept for an identity only if it is detected as that pad, with working rumble, where uinput is not. For the DualSense that is exactly what happened:

| Virtual pad | Kernel | SDL, default (HIDAPI) | SDL, evdev backend | Steam |
|---|---|---|---|---|
| uinput, DualSense name and IDs | plain input device | **not listed at all** | listed under the kernel name | **not adopted, nothing logged** |
| uhid, DualSense descriptor | `playstation` driver binds | "DualSense Wireless Controller", HIDAPI | listed under the kernel name | adopted, "Controller using HIDAPI driver" |
| uinput, Xbox Series IDs | plain input device | "Xbox Series X Controller", database mapping | same | adopted as "Xbox Series X Controller" |
| uinput, generic, no IDs | plain input device | "CtrlAssist Virtual Gamepad", generated mapping | same | adopted (seen in the legacy version's runs in the same log) |

- **This is why the legacy version never got PlayStation spoofing right.** SDL and Steam claim Sony's vendor and product for their hidraw driver and then ignore the evdev node carrying those IDs. A uinput pad has no hidraw node, so with Sony IDs it is invisible to both. Copying a DualSense's IDs onto a uinput pad is worse than not copying them.
- **A uhid pad built from a real DualSense's descriptor is accepted everywhere tested.** The kernel's PlayStation driver bound to it, SDL listed it beside the real one under the same name, and Steam opened it as a DualSense.
- **Xbox identities need nothing more than uinput.** SDL and Steam recognized the Xbox Series IDs on a uinput pad by name and gave it the database mapping.

## Rumble

Both kinds deliver it, in different forms:

- **uinput:** the kernel's force-feedback requests. A rumble test through SDL's evdev backend arrived as an effect upload (strong and weak magnitudes), then play and stop.
- **uhid:** raw HID output reports. A rumble test through SDL's default backend arrived as the DualSense's output report 0x02 with both motor bytes at full. The same report carries LEDs and player lights: the kernel driver and SDL each sent their own on connecting.

So feedback is two things, as the plan's provisional note anticipated: an effect protocol on uinput sinks, and device-specific output reports on uhid sinks. Passing a DualSense's output report through to a real DualSense is nearly a copy; routing it to a different pad means translating the motor bytes into a rumble effect.

## What a uhid identity costs

- **A per-identity emulator.** The spike's is about 150 lines and replays one captured input report with a button toggled. A real one must translate the graph's events into the DualSense's 64-byte input report, at the pad's reporting cadence, and parse its output reports.
- **Answering the driver's questions.** On binding, the kernel read three feature reports (pairing info, firmware version, calibration), all answered from the captured copies. The pairing report holds the pad's address; the spike gave the copy a different one, since the driver is understood to refuse a second pad with a known address (not tested).
- **Sony's report descriptor and feature reports as data.** The spike read them from the connected pad at run time. Shipping an identity that works with no real DualSense attached means carrying that data in the project. Other open-source projects do; whether this one may, and in what form, is a question for the maintainer.
- **`/dev/uhid` access,** which a clean machine doesn't give a seat user (see the hiding finding), so the packages' udev rule must grant it.
- **No bindgen dependency if the events are laid out by hand,** as both spikes do. The uhid protocol is nine event types.

On a clean machine the virtual DualSense's hidraw node would be closed to games, as a real one's is there, and they would read its evdev node instead. That node comes from the kernel's own driver, so it has the right name, IDs and layout either way.

## Default identity

The plan left the default to this spike. The evidence doesn't force one: the generic uinput pad and the Xbox one are both adopted by SDL and Steam. They differ in what a game shows:

- **Generic:** honest about being virtual and distinguishable from any real pad, but SDL builds its mapping from the pad's capabilities rather than the database, and a game has no known glyphs for it.
- **Xbox:** gets a known name, a database mapping and Xbox glyphs, which is what most PC games assume, at the cost of looking like real hardware.

Recommendation: generic by default, because a virtual pad that claims to be hardware it isn't should be a choice, with Xbox one setting away for games that need it. This is the maintainer's call.

## Not tested

- **Layout correctness under real input.** The pads were listed and mapped but no buttons were pressed against a game, so whether each mapped button lands where it should is unverified. The uinput fixture's button and axis set was chosen to match what the kernel drivers present, by reading, not by test.
- **Button glyphs,** which need a game's own interface. Detection by name stands in for them here.
- **Gyro, touchpad and LEDs** through the uhid pad. The descriptor and reports carry them; nothing read them.
- **A Bluetooth-style DualSense** (its reports carry a checksum), a **DualShock 4**, and any **Proton** game.
- **A real Xbox pad** for comparison, and Xbox pads over Bluetooth, which use a different kernel driver.

## What this changes in the plan

- Sinks come in two kinds, uinput and uhid, chosen by identity. The architecture keeps both.
- Feedback has two representations, and nodes that route it between unlike pads translate.
- "Copy a pad" can't mean copying IDs onto a uinput pad. For a PlayStation pad it means a uhid pad; for others, uinput with the pad's IDs.
- A uhid identity needs a unique address, which also serves as the marker by which the daemon recognizes its own virtual pads.
- The first release can ship Xbox and generic identities on uinput alone; the DualSense identity is a self-contained addition whose size is now known.
