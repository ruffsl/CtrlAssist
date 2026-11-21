# Rumble routing: the model chosen and the ones set aside

**Status:** decided for the first release; recorded so the choice can be revisited with its reasons in hand.

**Would bring it back:** support for wheels or other pads whose effects depend on position (spring, damper, friction); a game whose rumble feels wrong through CtrlAssist in a way the level model can't express; or evidence that the timer the level model needs costs something on the input thread.

## The problem

A game talks to a virtual pad through the kernel's force-feedback interface: it uploads effects into numbered slots (two motor strengths, a duration, an optional delay and repeat count), then plays and stops them by number, and can set an overall gain. CtrlAssist has to turn that into rumble on one or more physical pads, which may change while an effect is playing (a toggle hand-off, a pad dropping out), may be shared by two virtual pads (a demux), and may be a different kind of pad from the virtual one.

## Chosen: level-based, mirroring the kernel

The router keeps each game's effects and their timing itself. At any moment it computes one strong and one weak level per physical pad from everything playing that is routed to that pad, and keeps a single effect on each pad updated to that level.

- Within one virtual pad, playing effects add and clamp, after gain, as the kernel does.
- Across virtual pads sharing a pad, the strongest request wins on each motor.
- The input thread arms a timer for the next moment a level changes, and only while something is playing, so it still sleeps indefinitely when idle. The durations are the game's own.

What it settles:

| Situation | Outcome |
|---|---|
| A hand-off part-way through a rumble | The new pad gets the current level and it ends on time |
| A rumble that already ended | Contributes nothing; it can't be replayed by a later hand-off or reconnect |
| Two virtual pads on one pad | One slot on the pad, always; no slot exhaustion |
| A virtual pad that delivers levels natively (a HID-level DualSense reports its two motor strengths) | No conversion; it is already a level |
| A pad that refuses an update | One effect per pad to reason about, so a refusal leaves at most that effect stale and the next update retries |

## Set aside: per-effect forwarding

Each of the game's effects becomes an effect on each target pad, with uploads, plays, stops and erases forwarded one for one. The 0.4.0 release did this, and the routing spike's first router did too.

Why it was set aside:

- A playing flag has no end, so a hand-off or reconnect replays a rumble that finished long ago, unless the router tracks durations anyway.
- A hand-off part-way through a rumble can only restart it or drop it.
- Every game effect takes a slot on every target pad. A pad's driver offers 16, and two virtual pads sharing a pad compete for them; running out is a refusal the game sees.
- A changed effect that a pad refuses leaves the old one playing there.
- A virtual pad that delivers levels has to be converted into uploads and plays.

What it has going for it: the pad's own driver does the timing, so the daemon needs no timer, and effects that aren't rumble pass through unchanged. That second point is why it is not gone for good: a wheel's spring or damper depends on the wheel's position and can't be reduced to a level, so position-dependent effects would be forwarded per effect when such pads are supported, beside level-based rumble.

## Set aside: per-effect forwarding with durations

The same, but each forwarded effect carries its duration so the pad's driver ends it. It fixes the replay of finished rumbles and nothing else in the list above.

## Prior art

- **The Linux kernel**, `drivers/input/ff-memless.c`, read 2026-11-21: for every pad that can only rumble, it keeps up to 16 uploaded effects, combines the playing ones into one level (rumble strengths added and clamped, after gain), hands the driver that one level, and schedules a timer for the earliest next change. The Xbox and PlayStation drivers both use it. The chosen model is this logic in userspace, plus moving a level between pads, which the kernel never has to do.
- **Game-facing interfaces mostly speak levels already.** SDL's rumble call takes two strengths and a duration; XInput's takes two motor speeds; Wine keeps one kernel effect updated from those. This was written from memory and not re-checked.
- **A HID-level DualSense** delivers its motor strengths in its output report, observed in the identity work.
- **InputPlumber** is believed to forward rumble from its virtual pads as levels; its code was not read for this.

## Open within the chosen model

- Whether envelopes (attack and fade) are worth carrying. The kernel applies them to the periodic effects it converts into rumble; plain rumble effects have none.
- How gain and autocenter requests on a virtual pad are applied: gain as a multiplier per virtual pad; autocenter ignored for pads without it.
