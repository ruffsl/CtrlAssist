# A Haptics choice beside Rumble

A virtual Steam Controller passes the haptics a reader sends it, past rumble, to the Steam Controllers behind it: a pulse on a trackpad, a tone, a built-in effect, sound for the actuators ([Feedback](../architecture.md#feedback)). They go where rumble goes, by the profile's Rumble choice.

**The idea.** A Haptics choice beside Rumble, the same as Rumble by default, that could send a trackpad's haptics to whoever has a finger on that trackpad, which is whom a click is for, or to one pad, both or neither. Each report names its side, left, right or both, so "whoever touches it" can be told from the pad state as the report is passed.

**Why deferred.** It matters only with two real Steam Controllers behind a virtual one, and nobody has felt what is passed today. The maintainer chose the one rule first, on 2026-11-26.

**Would bring it in.** Two people with a Steam Controller each finding that the helper's hands buzz for the player's clicks.
