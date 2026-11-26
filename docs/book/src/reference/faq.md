# FAQ

## Background

### Who is CtrlAssist for?

Anyone who wants two people to share one game's controls, or to hand them back and forth without passing a controller:

- players with disabilities or limited mobility, and the people who play with them;
- beginners still learning a controller, and the experienced players helping them through a hard part;
- families with young children or older relatives who want to play together;
- anyone who would enjoy playing a single-player game as a pair.

### Why not just pass one controller around?

Handing a controller over works for turn-based games and for taking a level each. It falls short where help is wanted in the moment:

- it breaks the flow, for the player and for the game, while the controller changes hands;
- the player waits, and loses the thread, until control comes back;
- the player learns less, feeling none of what the game does while someone else holds the controller;
- the helper can only take over everything, when a little help with one stick was all that was needed.

With two controllers acting as one, the helper steps in on just the part that needs it, and lets go.

### Why was it made?

Out of need, at home. Consoles have had this for years: [Controller Assist on Xbox](https://support.xbox.com/en-US/help/account-profile/accessibility/copilot) and [Second Controller Assistance on PlayStation](https://www.playstation.com/en-us/support/hardware/second-controller-assistance/). After a household's gaming moved to Linux, the living room included, there was nothing like them. CtrlAssist began as a small command for that, and grew a tray and a window as it turned out to make family play better: someone who would have watched could take part.

## Games, Steam and Proton

### Does a game need to support CtrlAssist?

No. CtrlAssist works below the game: it presents one ordinary controller, and the game reads it as it would any other. A game needs no setting, and can't tell.

### Does it work with Steam and with Proton?

A virtual controller is an ordinary controller to the system, which is what Steam, SDL and games under Proton read. Each [identity](../guide/profiles.md#identity) has been recognized by Steam and by SDL, with rumble reaching the real controllers, and a game has been played through a virtual DualSense. Not every identity has been played in both a native game and one under Proton, and the motion sensors have been watched moving in Steam's controller test but not yet aimed with in a game. If something doesn't land where it should, [say so](https://github.com/CtrlAssist/CtrlAssist/issues).

### Why does my game see two controllers, or a press twice?

It sees a real controller beside the virtual one. A running profile keeps its real controllers from games, but a game or a launcher that was already running may have held on to one: start the profile first, then the game. [Troubleshooting](../operations/troubleshooting.md#the-game-sees-the-real-controllers-as-well-as-the-virtual-one) has the rest, the hiding service among it.

## Controllers and latency

### Which controllers work?

Anything Linux presents as a gamepad. Xbox and PlayStation controllers are read through their own drivers; others through the community controller database that SDL also uses. [Requirements](./requirements.md) has what has been tried, and [How It Works](./how-it-works.md#controller-layouts) how to add a layout. Keyboards, mice, wheels and flight sticks aren't combined.

### Does it add latency?

Not that you would feel. Measured between a real controller and the virtual one, the round trip is tens of microseconds, where a controller itself reports every few milliseconds. [How It Works](./how-it-works.md#latency) says how.

### Do the gyro and the touchpad carry over?

Yes, with a real DualSense or DualShock 4 behind a virtual controller of the **DualSense** identity, and the real one hidden. A game gets one controller's motion at a time, the primary's unless the profile gives the assist a [Gyro](../guide/profiles.md#gyro) button. With a real DualSense behind it, the game's lightbar, player lights and trigger effects reach it too. [Identity](../guide/profiles.md#identity) has the details.

## Setup and safety

### Does it need root?

No. The daemon, the tray, the window and the command all run as you. One small part, the hiding service, is a system service, and it too runs without root: as a user of its own, with no special capabilities, serving only the person sitting at the machine. Installing a package needs root, as any package does. [Security & Supply Chain](./security-supply-chain.md) says what runs with which rights.

### Where are my profiles kept?

In `~/.config/ctrlassist/profiles/`, one file each, which the window's **Show its file** opens. [How It Works](./how-it-works.md#what-is-kept-where) lists everything CtrlAssist keeps.

### Can two players each have a helper?

Yes: run two profiles, each with its own two controllers, and each makes its own virtual controller. A controller plays in one running profile at a time. One helper assisting two players in turn is possible too, as a profile written in the [full form](../guide/configuration.md#the-full-form).
