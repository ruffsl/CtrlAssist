# Finding: the input loop

Measured 2026-11-21 on the development host (32 logical CPUs) with `spikes/loop/` against the legacy 0.4.0 binary built from `legacy/0.4`. Reproduce with `just spike-loop-bench`.

## Verdict

**The loop is kept, and it reads pads directly through evdev. gilrs is not in the data path.**

The plan's rule was: no wakeups while idle, and no worse than legacy on latency tail and CPU in every measured condition. Both hold.

## The loop

One thread sleeps in `epoll_wait` with no timeout over three descriptors: the two pads and the virtual pad. A pad's events are forwarded as whole report frames; the virtual pad's feedback requests (effect upload, erase, play) are answered on the same thread before it sleeps again. About 200 lines over the `evdev` crate and libc's epoll.

## Measurements

The bench creates two fake pads through uinput, starts the target on them, watches it idle for five seconds, then for ten seconds sends 1,000 stick frames a second on one pad and 1,000 button edges a second on the other. Each stick frame is paired with the frame that leaves the target's virtual pad. Latency therefore spans the whole path: write to uinput, kernel, target, uinput again, kernel, read. Three runs per row; ranges are across runs. "Loaded" means 32 spinning threads beside the measurement. The bench's own threads run under real-time scheduling so the numbers reflect the target.

| Condition | Target | p50 | p99 | p99.9 | max | CPU | Idle wakeups/s |
|---|---|---|---|---|---|---|---|
| Idle host | new loop | 26–34 µs | 107–115 µs | 156–211 µs | 247–344 µs | 2.1–2.7% | **0** |
| | new loop, real-time | 29–32 µs | 43–56 µs | 112–129 µs | 151–215 µs | 2.4–2.6% | **0** |
| | legacy | 26–27 µs | 110–135 µs | 152–284 µs | 217–916 µs | 2.6–2.9% | **85–107** |
| Loaded | new loop | 6–8 µs | 9–13 µs | 419–1,008 µs | 994–2,017 µs | 0.3–0.5% | 0 |
| | new loop, real-time | 5–6 µs | 8–9 µs | **9–12 µs** | **24–78 µs** | 0.4–0.6% | 0 |
| | legacy | 6–9 µs | **548–864 µs** | 571–1,007 µs | 1,546–2,649 µs | 0.5–0.9% | 95–125 |

No frame was lost in any run (10,000 sent, 10,000 received).

Reading the table:

- **Idle cost is the clear difference.** Legacy wakes about a hundred times a second with no input at all; the new loop never wakes. On a machine running a game, and above all on a handheld, that is the number that matters.
- **On an idle host the two are alike** in the middle of the distribution. Tens of microseconds is the cost of the kernel round trip, not of either program.
- **Under load legacy's tail is much worse:** one frame in a hundred takes over half a millisecond, where the new loop's p99 stays near 10 µs. Legacy hands each event between threads; the new loop does everything on the thread that woke.
- **Under load the new loop still has a rare long wait** (around one frame in a thousand takes up to a millisecond, the worst 2 ms) because an ordinary thread can be descheduled. Real-time scheduling removes it: the worst frame in 30,000 was 78 µs.
- **Medians are lower under load** than on the idle host because busy CPUs never drop into sleep states. That is an artifact of the machine, not of the targets.

**Caveat:** the comparison is not like for like in work done. The new loop forwards frames unchanged; legacy runs gilrs's mapping and filters and its priority mode. The routing graph spike adds the missing work to the new loop and must show it stays small next to these numbers.

## Real-time scheduling

Worth having, not required. Without it the loop already beats legacy; with it the tail under load disappears.

This host allows it because its real-time priority limit is 95. A stock machine's limit is usually 0, so the daemon must either get the limit raised by its packaging (a limits file, or the system-wide user manager setting) or ask rtkit over D-Bus. Recommendation: request it at startup, carry on without it if refused, and report which in the daemon's status.

## What reads the pads

**Direct evdev.** Two findings from the other spikes and one from reading gilrs settle it without a measurement:

- **gilrs opens pads by path, itself.** With hiding on, the pad's nodes are closed to the user and the daemon receives open descriptors from the helper (hiding finding). gilrs has no way to be handed a descriptor. The `evdev` crate does (`Device::from_fd`).
- **gilrs owns the event loop,** so its events arrive through its own polling, which is where legacy's idle wakeups and thread hand-offs come from.
- **gilrs's mapping parser is private.** Its `mapping` module, which parses SDL's controller database, is not exported, so the database can't be applied through gilrs without also taking its reader.

What is lost is gilrs's normalization of oddly laid out pads. The pads the first release is verified with (Xbox and PlayStation families) get a standard layout from their kernel drivers and need none. For others, the SDL database's format is a documented line of text per pad; applying it means a small parser of our own over the same data file. That is recorded as work for the device crate, sized but not done here.

**`evdev` against `evdevil`:** not compared in code. `evdev` 0.13.2 had everything the loop needed (opening from a descriptor, grabbing, non-blocking reads, a reader that resynchronizes after dropped events, uinput with force feedback), so there was no reason found to take the newer, less used crate. `evdevil` stays a fallback if `evdev`'s release cadence becomes a problem.

## Feedback on the loop thread

A rumble test through SDL on the loop's virtual pad, with the real DualSense as the primary, completed normally and left the loop running, so the upload was answered and forwarded. Whether the pad physically shook was not observed. The upload is answered inline: the game's call blocks until it is, and the loop does nothing else in between.

## A lesson for the daemon

The first real-time runs failed intermittently. The cause was a `println!` in the loop process after the bench had stopped reading its output: the write hit a closed pipe and panicked, taking the virtual pad with it. This is the failure TisVlay's conventions guard against by forbidding `println!`. In the daemon it would mean a game losing its pad because a log reader went away, so that rule is not optional here.

## Not done

- **Suspend and resume, and a Bluetooth drop with the re-hide race.** Both need the maintainer's machine to be suspended or a pad to be power-cycled. With the helper's udev rule applied before the node appears (hiding finding), the re-hide race should not exist; that is reasoning, not observation.
- **A pad that needs a database mapping.** None was on hand.
- **Rates above 1,000 Hz per pad,** and more than two pads.

## What this changes in the plan

- The device crate is built on `evdev` and epoll, with pads taken as descriptors.
- gilrs leaves the dependency set. A parser for SDL's controller database is added to the device crate's work, for pads outside the Xbox and PlayStation families.
- The daemon requests real-time scheduling for the input thread and degrades without it.
- The zenoh spike has its baseline: the numbers above, and zero idle wakeups on the input thread.
