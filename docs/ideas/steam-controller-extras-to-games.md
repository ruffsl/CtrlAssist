# A Steam Controller's left trackpad and capacitive touch reaching a game

A Steam Controller (2026) pad is read whole: both trackpads, with touch and pressure, the four back buttons, and capacitive touch on both sticks and both grips are in the pad state. Only the right trackpad reaches a game, as a virtual DualSense's touchpad; the rest is read and dropped, and the back buttons serve only as a profile's own buttons, the Gyro button or a toggle's switch.

Ways they could reach a game:

- **A virtual Steam Controller,** which has a place for each ([its own idea](virtual-steam-controller.md)).
- **The left trackpad as a virtual DualSense's second contact.** A DualSense's touchpad takes two fingers, so the left trackpad's finger could be the second, placed on the left half. A game would read two fingers on one touchpad, a pinch or a scroll that nobody made, which is why the touchpad's rule never mixes two pads' fingers.
- **The back buttons as extra buttons of a generic virtual pad.** SDL's mapping for a generic pad can name four paddles; whether games read them from a generic pad, and Steam, is not known.

**Why deferred.** No identity has a place for them that a game would read as meant, and the one that has, the virtual Steam Controller, waits on the maintainer.

**Would bring it in.** The virtual Steam Controller, or someone asking for the back buttons in a game.
