# Steam Controller

Valve's Steam Controller, the one from 2026 with two trackpads and four back buttons, can play in a profile as a **Primary**, an **Assist** or a **Pad**, like any other controller. It needs a kernel recent enough to know it ([Requirements](../reference/requirements.md)).

**None of this has been tried on a real Steam Controller.** Nobody working on CtrlAssist has one yet: it was built and tested against a stand-in that the kernel's own driver takes for the controller on a USB cable. The Puck, the controller's wireless receiver, and Bluetooth have been tried only in part, by stand-ins whose shape is a guess. If it does something other than this page says, please [say so](https://github.com/CtrlAssist/CtrlAssist/issues).

**With Steam running, a Steam Controller in CtrlAssist is unreliable today.** [Steam](#steam) says why.

## What carries over

- **Every standard control:** the face buttons, the D-pad, the bumpers, the sticks and their clicks, the triggers, View and Menu as Select and Start, and the Steam button as the Mode (guide) button.
- **Rumble,** whatever the virtual controller's [identity](./profiles.md#identity).
- **Motion,** behind a **DualSense** identity, as a DualSense's does: the [Gyro](./profiles.md#gyro) choice decides whose.
- **The right trackpad,** behind a **DualSense** identity, as the virtual DualSense's touchpad, its click as the touchpad's click.
- **The four back buttons,** as the assist's button for **Gyro**: **Paddle 1** is the upper right, **Paddle 2** the upper left, **Paddle 3** the lower right and **Paddle 4** the lower left. A profile written by hand can also make one the switch that hands over in Toggle ([the full form](./configuration.md#the-full-form)). Games don't see them.

## What is dropped, and why

No identity a virtual controller can take has a place for these, so a game never gets them:

- the left trackpad, a DualSense having one touchpad;
- the capacitive touch of a thumb on a stick or a hand on a grip, and how hard a trackpad is pressed;
- the back buttons, the quick access button and the triggers' clicks, as buttons of their own;
- the controller's own haptics beyond plain rumble, such as a pulse on a trackpad, since what a game sends a controller carries none.

The lights and trigger effects a game sends a virtual DualSense don't reach it, as they don't reach any controller that is only grabbed ([Identity](./profiles.md#identity)).

## Grabbed, not hidden

The hiding service doesn't take a Steam Controller: it never hides a controller that can also type and move the pointer, as this one can, since hiding would cut those off too. So while a profile uses it, CtrlAssist reads it through its own raw interface, the way Steam does, which takes it from some programs and not others:

- **Games that read controllers the usual way** see only the virtual controller. The real one is gone from them while the profile runs, as a grabbed controller is.
- **Steam, and games that read the controller through SDL's own driver for it,** read the real controller too, beside the virtual one, and get each press twice. A game built on a recent SDL may read it that way without saying so.

**Visibility** keeps it from games the same way at **Hidden** and **Grabbed**, but that **Hidden** also asks Steam to leave the controller's model alone, as for any controller that is only grabbed; whether Steam honors that for its own controller isn't known, and Steam takes it up only when it next starts. The Pads page shows it as **Grabbed** while a profile uses it. **Visible** can't be chosen with it: the way CtrlAssist reads it takes it from games, so it can't be left to them, and a profile that asks for that is refused and says so.

## Steam

While Steam runs, it holds the controller's raw interface too, and that has three consequences:

- **Steam sees the real controller** beside the virtual one, as above, and a game played through Steam may take each press twice.
- **CtrlAssist can't take the controller again** while Steam holds it: a controller plugged in while Steam runs isn't listed at all, and one a profile let go of, as the profile stopped or a managed one restarted, can't be taken again until Steam is closed.
- **Steam writes its own settings to the controller** while both hold it. It turns the motion sensors off when it has no use for them, which CtrlAssist puts right only every few seconds, so the motion behind a virtual DualSense may drop out for a moment.

What you can do today is little: start the profile before Steam, or close Steam while you play a game started some other way. Whether CtrlAssist should hide this controller properly is an open question.

## Typing and pointing

A Steam Controller types and moves the pointer, which Valve calls lizard mode, whenever no program is using its raw interface: neither Steam, nor a game through SDL's own driver, nor CtrlAssist. A game that reads controllers the usual way doesn't turn it off. While a profile uses the controller, CtrlAssist keeps lizard mode off, and it comes back as soon as the profile stops. If CtrlAssist hangs while using it, the controller turns lizard mode back on by itself.

## Identify

**Identify** buzzes a Steam Controller while a running profile is using it. Otherwise it can buzz it only where the system's kernel was built with rumble for it, which some aren't, NixOS's among them; there the window says the controller can be buzzed while a profile uses it.

## Naming it

A Steam Controller's **Pad ID**, on the Pads page under **INFO**, is `address:` and its serial number, in letters and digits, the same on a cable, through the Puck and over Bluetooth. A profile names it by that full ID ([Configuration Files](./configuration.md#naming-a-controller) has the ways to name a controller). Its **Model** is another on each of those, so naming it by model takes it on one of them only.
