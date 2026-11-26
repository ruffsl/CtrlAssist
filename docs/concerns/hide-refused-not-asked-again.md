# A hide refused for a moment is not asked again

A pad the hiding service refuses to hide is grabbed instead, and the daemon notes that it wants hiding (see [the control side](../architecture.md#the-control-side)). Where the refusal is one that would pass, as a session that isn't in front at that moment is, the hide is not asked again until the pad next returns or its setup restarts: the pad stays grabbed for the rest of the setup.

What would end such a refusal is the user's session coming back in front, which the daemon doesn't watch. Asking again at each piece of work the daemon does was a loop, a hide asked without end where the service couldn't be reached, and was taken out.

**Risk.** Low. A grabbed pad still works in its setup; a game that reads pads' event nodes sees a second pad that says nothing, and the tray and `ctrlassist setups` say the pad is grabbed.

**Mitigation.** Stopping and starting the profile asks again.

**Closes when.** The daemon hears of its session coming in front, from logind, and asks then for what it holds grabbed; or the refusal is found never to happen to a setup a user started from their own session.

A joystick node hidden late is [its own concern](joystick-node-hidden-late.md), and a touchpad shown without a re-bind [another](touchpad-shown-without-rebind.md).
