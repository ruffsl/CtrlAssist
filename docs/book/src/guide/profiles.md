# Profiles

A profile is an arrangement you keep: which controllers take part, and how they are combined. A profile that is running is what puts a virtual controller in front of your games. You make and change profiles on the window's **Profiles** page or in the tray's menu; this page says what each choice means, by the labels those show.

Each profile is also a short file, which you can write by hand, and which can describe arrangements the window's fields can't. [Configuration Files](./configuration.md) is the reference for that.

## Mode

How the controllers are combined. Three modes make two controllers into one, a **Primary**, the main player's, and an **Assist**, the helper's:

- 👑 **Priority**, the default: the primary plays, and the assist takes over whatever it touches, for as long as it touches it.
  - A stick or the D-pad is the assist's while the assist has moved it, and the primary's the rest of the time. A button is pressed while either presses it, and a trigger takes whichever is pulled further.
  - For help that comes and goes: the helper takes the camera, or the hard jump, and lets go.
- ⚖️ **Average**: the two are blended.
  - A stick or a trigger that both are using sits halfway between them; one that only one is using is that one's. A button or a D-pad direction is pressed while either presses it.
  - For steering together and gentle corrections, as with a counter-steer or a little braking in a racing game.
- 🔃 **Toggle**: one plays at a time.
  - Everything comes from whoever has control. The Mode (guide) button on the assist's controller hands over, and so does **Has control** in the window or the tray.
  - For when two sets of hands would only get in each other's way: menus, or a precise stretch one player does alone.

Two modes go the other way, and make one controller, the **Pad**, into several virtual ones:

- 🔂 **Unicast**: the controller plays one virtual controller at a time, and its Mode button moves it to the next.
- 📢 **Multicast**: the controller plays all of them at once.

**Outputs** is how many virtual controllers those two make.

## Inputs

**Primary** and **Assist**, or **Pad**, are the controllers that take part. The window and the tray list the ones CtrlAssist knows; one that is unplugged, or busy in another running profile, can't be picked. Picking for one part the controller the other part has swaps the two, so the player and the helper change places in one pick.

A part can be given one particular controller, or any controller of a model; the second is handy, and can't tell two identical controllers apart. [Configuration Files](./configuration.md#naming-a-controller) has the ways to write each. Once a part has taken a controller, it waits for that same one if it disconnects.

A profile whose controllers aren't chosen yet is kept and shown as **Not set up**. It isn't wrong; it starts once they are chosen.

## Identity

What the virtual controller says it is to games:

| Identity | Appears as |
|---|---|
| **Generic**, the default | A plain gamepad that games map from its buttons. |
| **Xbox** | An Xbox Series controller, which most games recognize outright. |
| **DualSense** | A PlayStation 5 DualSense controller, with PlayStation button prompts in games that have them. |

If you connect a real DualSense by USB cable, with the hiding service installed, CtrlAssist copies what that controller says of itself, keeps the copy in `~/.local/state/ctrlassist/identities/`, and makes its virtual DualSenses from that from then on; until then it uses a description of the controller that ships with it. You don't need to own one.

A virtual DualSense carries the buttons, sticks, triggers and rumble. Its touchpad and motion sensors report those of a hidden DualSense or DualShock 4 behind it, and nothing with any other pad or one that isn't hidden. With two controllers, the touchpad is the assist's while the assist has a finger on it in Priority, whoever's touched it first in Average, and the controller's that has control in Toggle; the motion sensors follow [Gyro](#gyro). The touchpad moves the desktop's pointer as a real DualSense's does, and the system shows a battery and lights for it, as it does for a real one.

When the controller behind it is a real DualSense, on a USB cable or over Bluetooth, and the hiding service has hidden it ([Visibility](#visibility) left at Hidden, with the service installed), the game's lightbar, player lights and adaptive trigger effects reach it too. A DualShock 4 behind it, either way, gets the lightbar's color. A controller that is only grabbed, or not hidden, gets only the rumble. A controller that a game-streaming host presents as a DualSense is treated as one; whether the lights and trigger effects then reach the controller at the far end depends on the host.

## Rumble

Who feels what a game rumbles, in the modes of two controllers: **Both pads**, the default, **Primary only**, **Assist only** or **Neither**. A helper who only steps in now and then may not want the rumble; a player who can't feel it may want the helper to.

## Gyro

Whose motion sensors a game gets, in Priority and Average. A game reads one controller's gyroscope and accelerometer, never a blend, so that the two agree.

- **Primary only**, the default: always the primary's.
- **Assist**, with a button: the assist's while the assist holds that button of their own controller, and the primary's again when they let go. The game doesn't see that button of the assist's; the primary's same button is untouched.

In Toggle the sensors follow whoever has control, and there is no such choice. Only a virtual controller with the DualSense identity carries motion.

## Visibility

How the real controllers are kept from games while the profile runs, so that a game doesn't get every press twice:

| Visibility | Do games see the real controllers? |
|---|---|
| **Hidden**, the default | No. The hiding service takes them away; where it isn't installed they are grabbed, and Steam is told to leave them alone. |
| **Grabbed** | Most don't. A program that reads controllers another way, as Steam does, still sees them. |
| **Visible** | Yes, beside the virtual one. Use it when a game should read a real controller directly, and expect a game that reads both to take a press twice. |

The Pads page shows the same three words for each controller a running profile is using: what came of it, which is **Grabbed** where Hidden was asked and the hiding service isn't there. [How It Works](../reference/how-it-works.md#hiding) says what the hiding service does.

## Managed

A profile is started by hand unless it is **Managed**. A managed profile is started by CtrlAssist whenever every controller it names is connected: when CtrlAssist starts, when the profile is kept, and when one of its controllers is plugged in. If you stop it by hand it stays stopped until one of its own controllers is plugged in again.

## The default profile

CtrlAssist always keeps one profile, `default`: it is there from the first start, with its controllers still to choose, so there is always a profile to fill in from the tray or the window. It is listed first, with a check mark. Change it as you like; it can't be deleted or renamed, and if its file is removed it is made again.

A profile whose controllers aren't chosen yet is kept and shown as **Not set up**. It isn't wrong; it starts once its controllers are chosen. `ctrlassist profile list` says which ones those are.

## Changing a running profile

What you pick in the window or the tray is kept at once, and a running profile can take the change up.

- **As it runs,** where the virtual controllers stay as many and of the same identities: other controllers, another mode, who feels rumble, the Gyro button. A game notices nothing. A controller that is swapped out is given back to the system and the new one hidden, and a Toggle goes on with whoever had control.
- **By a restart,** where a virtual controller has to be made anew: another identity, another number of outputs, or another Visibility. A game sees its controller disconnect and another connect.

The window says which it will be before it does it, and the tray says afterwards. From a terminal, `ctrlassist update` does the same; see [Configuration Files](./configuration.md#changing-a-running-setup).
