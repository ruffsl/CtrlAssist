# Profiles

A profile is a short TOML file that describes an arrangement: which controllers take part and how they are combined. The daemon keeps profiles by name and runs them as setups.

This page says what a profile can do. How one is written as a file, and the commands that keep and change them, are on [Configuration Files](./configuration.md).

## Identity

`identity` is what the virtual controller says it is to games:

| Identity | Appears as |
|---|---|
| `generic` (the default) | A plain gamepad that games map from its buttons. |
| `xbox` | An Xbox Series controller, which most games recognize outright. |
| `dualsense` | A PlayStation 5 DualSense controller, with PlayStation button prompts in games that have them. |

`ctrlassist identities` lists them and whether each can be made on this machine.

If you connect a real DualSense by USB cable, with the hiding service installed, CtrlAssist copies what that controller says of itself, keeps the copy in `~/.local/state/ctrlassist/identities/`, and makes its virtual DualSenses from that from then on; until then it uses a description of the controller that ships with it. You don't need to own one.

A virtual DualSense carries the buttons, sticks, triggers and rumble. Its touchpad and motion sensors report those of a hidden DualSense or DualShock 4 behind it, and nothing with any other pad or one that isn't hidden. With two controllers, the touchpad is the assist's while the assist has a finger on it in `priority`, whoever's touched it first in `average`, and the controller's that has control in `toggle`; the motion sensors are the primary's unless `assist-gyro` says otherwise. The touchpad moves the desktop's pointer as a real DualSense's does, and the system shows a battery and lights for it, as it does for a real one.

When the controller behind it is a real DualSense, on a USB cable or over Bluetooth, and the hiding service has hidden it (`hide` left at `full`, with the service installed), the game's lightbar, player lights and adaptive trigger effects reach it too. A DualShock 4 behind it, either way, gets the lightbar's color. A controller that is only grabbed, or not hidden, gets only the rumble. A controller that a game-streaming host presents as a DualSense is treated as one; whether the lights and trigger effects then reach the controller at the far end depends on the host.

## The default profile

CtrlAssist always keeps one profile, `default`: it is there from the first start, with its controllers still to choose, so there is always a profile to fill in from the tray or the window. It is listed first, with a check mark. Change it as you like; it can't be deleted or renamed, and if its file is removed it is made again.

A profile whose controllers aren't chosen yet is kept and shown as **Not set up**. It isn't wrong; it starts once its controllers are chosen. `ctrlassist profile list` says which ones those are.

## Managed profiles

A profile is started by hand unless it is managed. With `managed = true`, the daemon starts the profile whenever every controller it names is connected: when the daemon starts, when you put the profile, and when one of its controllers is plugged in. If you stop it by hand it stays stopped until one of its own controllers is plugged in again.
