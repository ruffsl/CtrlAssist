# A virtual Steam Controller's haptics are passed on unfelt, its sound unpaced

A reader of a virtual Steam Controller, Steam or a game through SDL, sends it haptics as output reports, and the daemon passes each, whole, to the Steam Controller pads behind it that the profile's Rumble choice names ([Feedback](../architecture.md#feedback)). What they are is read, not seen:

- **`0x81` to `0x85`,** a pulse, a command, a tone, a sweep and a built-in effect, each with the side it is for: structs in SDL's headers, a snapshot of Valve's from November 2024, with the sizes SDL itself takes them at, and their numbers in the kernel's driver, which sends the first.
- **`0x86` to `0x88`,** which carry sound for the pad's actuators: a report that sets the stream up and stops it, and 8 kHz 16-bit sound, mono and stereo, by two other projects' captures of Steam with a real pad.
- **`0x89`,** which a real pad's descriptor declares at 64 bytes and nobody has published the meaning of. It is passed too, the maintainer's choice on 2026-11-26: every haptic report, and nothing else a reader writes.

**What only the pad can say:**

- **Whether any of it is felt as sent,** and on the side sent. The kernel's driver notes that left and right are swapped in the pulse "for legacy reasons"; the daemon passes the side as the reader wrote it.
- **Whether sound holds together.** It is a stream of some 250 reports a second, and nothing here paces it: each report is written as it comes. The pad's buffer and its timing are unmeasured by anyone. Where the writer is held up, the oldest reports are let go, 16 kept, which is right for a click and makes a gap in a sound.
- **What two pads make of one stream,** where Rumble names both: twice the writes, each pad at its own pace.
- **What `0x89` does.**

**What is known.** Against two fakes, a pulse and a report of sound written to the virtual pad's raw node reach the fake the Rumble choice names, whole and in order, and not the other. The reports' sizes are declared in the virtual pad's descriptor as a real pad declares them.

**Risk.** Low for the first five, which are short effects a pad either plays or doesn't. Medium for sound: broken-up sound from a pad's actuators is unpleasant, and a pad sent a stream at the wrong pace may do something else again. Unknown for `0x89`.

**Mitigation.** A profile whose Rumble is Neither passes nothing.

**Closes when** a pad behind a virtual Steam Controller has been felt and heard with Steam driving it: Steam's own haptics test of the virtual pad, for a click on each trackpad on the side Steam names; a game or Steam setting that plays sound through the controller, for whether it is whole; the same with two pads and Rumble at Both; and `strace -f -e trace=write -p <daemon>` meanwhile, for whether `0x89` is ever written and what follows it.
