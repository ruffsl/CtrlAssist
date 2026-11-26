# A virtual DualSense was refused by the kernel's driver once, on a machine with every core busy

A setup with the DualSense identity failed to start once: "making the virtual pad `virtual` (dualsense): the kernel's PlayStation driver refused the pad". That is what the emulator says when the driver starts on the pad and lets it go before it is bound, as it does for an address it already has ([virtual pads](../architecture.md#virtual-pads)).

**What was seen.** On 2026-11-26, with all 32 cores of the development host kept busy by loops beside the whole test suite, six runs in a row: once, in the SDK's `steam::a_paddle_named_as_the_gyro_button_hands_the_motion_over`, whose daemon has a home of its own and so an address no other pad has. The other five runs made every virtual DualSense the suite makes. It was not seen on an idle machine in sixteen runs.

**What isn't known.** Why the driver let the pad go. It reads three feature reports from the pad as it binds, each answered by the emulator, and gives up on one that isn't answered in time; a daemon kept off the processor for that long would be refused so. That is a guess: the kernel's log of that run was not read.

**Risk.** Low. A machine so busy that the daemon doesn't run for seconds has worse troubles, and the start fails with a reason and can be asked again.

**Closes when** the cause is read from the kernel's log: run the suite with every core busy (`for i in $(seq $(nproc)); do (while :; do :; done) & done`, then `just test`, with Steam closed) until it shows, and read `journalctl -k` for what `playstation` said of the pad at that moment. If it is the driver's wait for a feature report, whether the emulator's answers should come from a thread that can't be starved is the question to bring to the maintainer; a longer wait isn't the daemon's to give.
