# FAQ

## Background

### Who is CtrlAssist for?

CtrlAssist is designed for anyone who wants to combine multiple controllers into one, enabling collaborative play, real-time assistance, or better gamepad ergonomics. Ideal for accessibility and partial asynchronous input, i.e. offloading camera angle management, movements requiring speed and precision, or on standby backup during difficult combat encounters. CtrlAssist is especially useful for:

- Players with disabilities or motor skill challenges
- Beginners still developing muscle memory for controller gameplay
- Experienced gamers helping newcomers through challenging games
- Families with young children or older relatives who want to play together
- Anyone interested in collaborative co-op for single or multiplayer games

### Why not pass around a single controller?

While playing "hot potato" with a gamepad may be sufficient for some scenarios, like turn-based games, divvying up level progression, menu navigation, the approach falls short for many reasons:

- Broken immersion
  - Context switching in real life pulls players out of the story
- Added friction
  - Awkward handoffs and delays interfere with real-time gameplay
- Reduced agency
  - Waiting for returned control can kill spontaneity and focus
- Deprived learning
  - No haptic feedback hinders recognizing cues like attack telegraphs
- Marginal assistance
  - Inhibited intervention may only compound unnecessary frustration

Accessibility features such as controller assist address these issues by enabling simultaneous/partial input from multiple controllers, resulting in more fluid and engaging gameplay.

### Why was CtrlAssist developed?

CtrlAssist was first created out of personal necessity. After migrating household gaming to Linux, including the family living room, the lack of controller assist features found on consoles like [Xbox](https://support.xbox.com/en-US/help/account-profile/accessibility/copilot) and [PlayStation](https://www.playstation.com/en-us/support/hardware/second-controller-assistance/) became clear. CtrlAssist was developed as an open source solution to make group gaming sessions on PC more inclusive and accessible for players of all ages and abilities.

Following its initial release and personal household success, as well as the broader trend of Linux adoption, CtrlAssist evolved from a simple CLI tool into a desktop-friendly utility. This category of accessibility features has significantly enhanced family gaming time, transforming passive spectators into active participants. From helping grandparents experience new immersive and interactive single player stories, to leveling age gaps across nieces and nephews in multiplayer PvPs, to rescuing friends from critical damage and finally overcoming a challenging boss, assistive players may expect as much enjoyment as primary players.

## Games, Steam and Proton

### What games are compatible?

CtrlAssist works with most Linux games that support standard gamepad input, and a game needs no support or setting for it: it reads the virtual controller as it would any other. Some games or launchers may require restarting after a profile starts or its virtual controller changes. Note that many games have no explicit setting for controller selection, thus the motivation for [hiding](../guide/profiles.md#visibility) the real controllers to avoid input conflicts between physical and virtual devices.

Even in games that natively support multiple controllers, simultaneous input from multiple devices is often not handled. Most games prioritize one controller at a time, only switching after a period of inactivity. CtrlAssist overcomes this limitation by merging inputs into a single virtual device and providing advanced multiplexing [modes](../guide/profiles.md#mode) for input events, going beyond simple first-come, first-served behavior.

### Does it work with Steam and with Proton?

A virtual controller is an ordinary controller to the system, which is what Steam, SDL and games under Proton read. Each [identity](../guide/profiles.md#identity) has been recognized by Steam and by SDL, with rumble and other feedback such as trigger and color effects reaching the real controllers.

### Why does my game see two controllers, or a press twice?

It sees a real controller beside the virtual one. A running profile keeps its real controllers from games, but a game or a launcher that was already running may have held on to one: start the profile first, then the game. [Troubleshooting](../operations/troubleshooting.md#the-game-sees-the-real-controllers-as-well-as-the-virtual-one) has the rest, the hiding service among it.

## Controllers and latency

### Which controllers are supported?

CtrlAssist supports most standard gamepads, such as those with a conventional Xbox or PlayStation layout, including those with strong and weak force feedback (rumble) capabilities. Xbox and PlayStation controllers are read through their own drivers, and others through the community controller database that SDL also uses. [Requirements](./requirements.md) has what has been tried, and [How It Works](./how-it-works.md#controller-layouts) how to add a layout.

Specialized controller features such as adaptive triggers, the lightbar, the touchpad, and gyroscopic and accelerometer motion tracking are supported for a DualSense and DualShock 4; [see below](#do-the-gyro-and-the-touchpad-carry-over). Other controllers' own extras are not yet supported. If you have device driver expertise and would like to contribute support for additional controller features, please consider opening a pull request!

### Are mice or keyboards supported?

Not directly, as CtrlAssist is focused on gamepad input multiplexing; wheels and flight sticks aren't combined either. However, it may be possible to combine CtrlAssist with more advanced utilities such as [InputPlumber](https://github.com/ShadowBlip/InputPlumber) to route keyboard and mouse events to virtual gamepads and into CtrlAssist, or vice versa taking virtual gamepads from CtrlAssist to keyboard and mouse events. The two shouldn't manage the same controller, and the pairing hasn't been tried.

Note that mouse and keyboard inputs are typically handled differently from gamepad inputs, as they are core interfaces for operating systems and display managers. Merging events from multiple mice and keyboards is often managed by the OS already, negating the need for simpler multiplexing software.

### Does it add latency?

Not that you would feel. Measured between a real controller and the virtual one, the round trip is tens of microseconds, where a controller itself reports every few milliseconds. [How It Works](./how-it-works.md#latency) says how.

### Do the gyro and the touchpad carry over?

Yes, with a real DualSense or DualShock 4 behind a virtual controller of the **DualSense** identity, and the real one hidden. A game gets one controller's motion at a time, the primary's unless the profile gives the assist a [Gyro](../guide/profiles.md#gyro) button. With a real DualSense behind it, the game's lightbar, player lights and trigger effects reach it too. [Identity](../guide/profiles.md#identity) has the details.

## Setup and safety

### Does it need root?

No. The daemon, the tray, the window and the command all run as you. One small part, the hiding service, is a system service, and it too runs without root: as a user of its own, with no special capabilities, serving only the person sitting at the machine. Installing a package needs root, as any package does. [Security & Supply Chain](./security-supply-chain.md) says what runs with which rights.

### Where are my profiles kept?

In `~/.config/ctrlassist/profiles/`, one file each, which the window's **Show its file** opens. [How It Works](./how-it-works.md#what-is-kept-where) lists everything CtrlAssist keeps.

### Is running multiple profiles possible?

Yes! For scenarios where multiple primary players would like assistance, such as true split-screen multiplayer, multiple profiles can be run simultaneously. Each profile will create its own virtual gamepad device, all from the one system tray icon and menu.

Additionally, each profile can use different visibility, identity, and rumble options to suit the needs of each player. Just be mindful that a controller plays in one running profile at a time. One helper assisting several players in turn is possible too, as a profile written in the [full form](../guide/configuration.md#the-full-form).
