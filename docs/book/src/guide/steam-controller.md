# Steam Controller

Valve's Steam Controller, the one from 2026 with two trackpads and four back buttons, can play in a profile as a primary, an assist or a shared controller, like any other. It needs Linux 7.3 or newer, the first release whose driver knows it ([Requirements](../reference/requirements.md)).

**None of this has been tried on a real Steam Controller.** Nobody working on CtrlAssist has one yet: it was built and tested against a stand-in that the kernel's own driver takes for the controller on a USB cable. The Puck, the controller's wireless receiver, and Bluetooth haven't been tried even that way. If it does something other than this page says, please [say so](https://github.com/CtrlAssist/CtrlAssist/issues).

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

The lights and trigger effects a game sends a virtual DualSense aren't sent on to it, as to any controller that isn't a DualSense.

## Grabbed, not hidden

The hiding service doesn't take a Steam Controller: it never hides a controller that can also type and move the pointer, as this one does when no game is running, since hiding would cut those off too. So while a profile uses it, CtrlAssist reads it the way Steam does, which takes it from some programs and not others:

- **Games that read controllers the usual way** see only the virtual controller. The real one is gone from them while the profile runs, as a grabbed controller is.
- **Steam, and games that read the controller through SDL's own driver for it,** read the real controller too, beside the virtual one, and get each press twice. A game built on a recent SDL may read it that way without saying so.

What you can do about the second today is little. Where Steam is running, it sees the real controller whatever CtrlAssist does. A profile left at **Hidden** asks Steam to leave the controller's model alone, as it does for any controller that is only grabbed, but whether Steam honors that for its own controller isn't known, and Steam takes it up only when it next starts. Whether CtrlAssist should hide this controller properly is an open question.

**Visibility** keeps it from games the same way at **Hidden** and **Grabbed**, but for that word to Steam, and the Pads page shows it as **Grabbed** while a profile uses it. **Visible** can't be chosen with it: the way CtrlAssist reads it takes it from games, so it can't be left to them, and a profile that asks for that is refused and says so.

If Steam is running when the controller is plugged in, Steam takes it first and CtrlAssist doesn't see it at all until Steam lets go: close Steam, or start the profile before Steam.

## Typing and pointing

With no game running, a Steam Controller types and moves the pointer, which Valve calls lizard mode. While a profile uses it, CtrlAssist keeps that off, and it comes back as soon as the profile stops. If CtrlAssist hangs while using it, the controller turns it back on by itself after a few seconds.

## Identify

**Identify** buzzes a Steam Controller while a profile uses it. When none does, it can buzz it only where the system's kernel was built with rumble for it, which many distributions' aren't; where it wasn't, the window says the controller can be buzzed while a profile uses it.

## Naming it

`ctrlassist pads` gives a Steam Controller an address that is its serial number, in letters and digits, which is the same on a cable, through the Puck and over Bluetooth. A profile names it by that address, as for [any controller](./configuration.md#naming-a-controller). Its model is another on each of those, so a `model:` name takes it on one of them only.
