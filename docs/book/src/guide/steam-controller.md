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

Behind any identity but [its own](#as-an-identity), these have no place, so a game never gets them:

- the left trackpad, a DualSense having one touchpad;
- the capacitive touch of a thumb on a stick or a hand on a grip, and how hard a trackpad is pressed;
- the back buttons, the quick access button and the triggers' clicks, as buttons of their own;
- the controller's own haptics beyond plain rumble, such as a pulse on a trackpad, since what a game sends any other controller carries none.

The lights and trigger effects a game sends a virtual DualSense don't reach it, as they don't reach any controller that is only grabbed ([Identity](./profiles.md#identity)).

## As an identity

A profile's **Identity** can be **Steam Controller**: the virtual controller then says it is one, on a cable, and carries what only this controller has. Both trackpads, the four back buttons, the quick access button and the touch of a thumb on a stick or a hand on a grip reach the game as a real one's do. A controller that isn't a Steam Controller can be behind it too: a DualSense's touchpad is its right trackpad, and what the controller lacks stays at rest. With two controllers, each trackpad is one player's at a time, as a DualSense's touchpad is, and the touches of thumbs and hands come from whoever's motion the game is getting ([Gyro](./profiles.md#gyro)).

It is for Steam, and for games that read a Steam Controller as Steam does, which those built on a recent SDL do. **Any other game gets nothing from it.** The system keeps every Steam Controller silent to such games until the player holds its Menu button for half a second, a virtual one included, and CtrlAssist leaves that as it is. For those games choose **Xbox** or **DualSense**.

Steam takes a virtual Steam Controller as its own, as it does a real one: it applies its own configuration to it and shows the game a controller of Steam's making. The virtual one keeps the same serial number each time a profile runs, so Steam keeps one configuration for each profile. What Steam writes to it, its settings and any firmware it offers, stays in the virtual controller and never reaches a real one. Its haptics do: the clicks, tones and effects Steam or a game sends the virtual controller's trackpads go on to each real Steam Controller the profile's [Rumble](./profiles.md#rumble) choice names, as they were sent.

It needs Linux 7.3 or newer, as a real one does. **Nobody has tried it with Steam yet**: the system's own driver and SDL take it for the controller, and that is all that has been seen.

## Visibility

CtrlAssist reads a Steam Controller through its own raw interface, the way Steam does. While a profile uses it, games that read controllers the usual way see only the virtual controller, whatever the profile's **Visibility**. What Steam sees, and games that read the controller through SDL's own driver for it, depends on how it is connected:

- **On its USB cable, at Hidden, with the hiding service installed,** it is hidden from them too, as any controller is. Its keyboard and pointer ([below](#typing-and-pointing)) stay listed by the system, and say nothing while the profile runs.
- **Behind its Puck or over Bluetooth, at Grabbed, or without the hiding service,** they read the real controller too, beside the virtual one, and get each press twice. A game built on a recent SDL may read it that way without saying so. At **Hidden**, CtrlAssist also asks Steam to leave the controller's model alone, as for any controller that is only grabbed; whether Steam honors that for its own controller isn't known, and Steam takes it up only when it next starts.

The Pads page shows which came of it, **Hidden** or **Grabbed**. **Visible** can't be chosen with it: the way CtrlAssist reads it takes it from games, so it can't be left to them, and a profile that asks for that is refused and says so.

## Steam

While Steam runs, it holds the raw interface of every Steam Controller it can open, and that has three consequences:

- **CtrlAssist can't take a controller Steam holds:** one plugged in while Steam runs isn't listed at all, and one a profile let go of, as the profile stopped or a managed one restarted, can't be taken again until Steam is closed. A controller a profile has hidden is out of Steam's reach for as long as the profile runs.
- **Steam sees a real controller that isn't hidden** beside the virtual one, as above, and a game played through Steam may take each press twice.
- **Steam writes its own settings to a controller that isn't hidden** while both hold it. It turns the motion sensors off when it has no use for them, and CtrlAssist asks for them only as a profile starts, so the motion behind a virtual DualSense may stop until the profile is started again.

What you can do: connect the controller by its cable, with the hiding service installed, and start the profile before Steam, so that the controller is hidden by the time Steam looks for it. Otherwise, close Steam while you play a game started some other way.

## Typing and pointing

A Steam Controller types and moves the pointer, which Valve calls lizard mode, whenever no program is using its raw interface: neither Steam, nor a game through SDL's own driver, nor CtrlAssist. A game that reads controllers the usual way doesn't turn it off. CtrlAssist turns lizard mode off as a profile takes the controller, and the system turns it back on as soon as the profile stops, or CtrlAssist ends for any reason.

## Identify

**Identify** buzzes a Steam Controller while a running profile is using it. Otherwise it can buzz it only where the system's kernel was built with rumble for it, which some aren't, NixOS's among them; there the window says the controller can be buzzed while a profile uses it.

## Naming it

A Steam Controller's **Pad ID**, on the Pads page under **INFO**, is `address:` and its serial number, in letters and digits, the same on a cable, through the Puck and over Bluetooth. A profile names it by that full ID ([Configuration Files](./configuration.md#naming-a-controller) has the ways to name a controller). Its **Model** is another on each of those, so naming it by model takes it on one of them only.
