# The Tray

The tray is the everyday way to use CtrlAssist. Its icon shows trouble at a glance: a warning when an arrangement is waiting for a controller, an error when one has failed. Hovering over it lists what runs: each arrangement with its mode, its controllers, a ● beside the one that has control, and its virtual controller. After them come the profiles that are ready to start, and when nothing runs, how many controllers are connected.

A **middle click** on the icon starts or stops one profile without opening the menu: the one marked ► in that list. That is the profile you last started or stopped, and scrolling the **mouse wheel** over the icon moves the mark to another. The icon and the menu's icons come from your desktop's icon theme, in its flat single-color style where the theme has one, so they follow a light or dark panel.

## The menu

**Profiles** come first: each one you have kept, and anything running.

- A profile that isn't running offers **Start**.
- A running one shows its state in brackets and offers **Stop**. Under it are its controllers (and which is disconnected, if one is), its virtual controller, and for a toggle, **Has control**: who plays now, and a click to hand over. A controller that isn't connected can't be handed to.
- One that has failed says why. **Stop** clears it.

**Quick start** pairs two controllers without writing anything; see the [Quick Start](../getting-started/quick-start.md). It keeps one profile, `quick`, which it rewrites each time. While `quick` runs, what you pick takes effect at once, without the game noticing: other controllers, a new mode, who feels rumble, starting by itself. A controller you swap out is given back to the system, and the new one is hidden. Picking another identity takes effect at once too, but needs a new virtual controller, which a game sees as its controller disconnecting and another connecting. A controller that is unplugged, already chosen for the other part, or busy in another arrangement can't be picked.

**Pads** lists every controller CtrlAssist knows, connected or remembered, and after them the virtual controllers that exist now.

- **Identify** buzzes a connected controller for a moment, so you can tell two apart. If a game is rumbling it, the buzz takes over briefly.
- **Identify** on a virtual controller rumbles it as a game would, so the controllers behind it buzz the way its profile passes rumble on: a quick check of who is mapped to it and who feels rumble.
- **Forget** drops a controller that isn't connected from the list.

The last line says how controllers are being hidden (`service` or `grab`) and whether the daemon got real-time scheduling. [How It Works](../reference/how-it-works.md) explains both.

**Settings** opens the CtrlAssist window, or brings it to the front if it is open. A left click on the tray's icon does the same.

## Notifications

CtrlAssist tells you when:

- an arrangement **started** or **stopped**;
- an arrangement is **held** because a controller disconnected, and when it is **running again**;
- an arrangement **failed**;
- the **daemon stopped**, which removes every virtual controller, or came back;
- an arrangement started with controllers that are **only grabbed, not hidden**, or with one that **can't rumble**;
- a controller that no arrangement uses **connected** or **disconnected**;
- something you asked for from the menu **couldn't be done**, with the reason.

Each kind can be turned off. The switches are kept in `tray.toml`, in `~/.config/ctrlassist` beside your profiles, and everything is on until you turn it off:

```toml
schema = 1
setup_started = true    # an arrangement started or stopped
setup_trouble = true    # held, failed, only grabbed, no rumble, the daemon stopped
pad_connection = true   # a controller connected or disconnected
refused = true          # something asked from the menu couldn't be done
```

The tray reads the file when it starts. After editing it by hand, restart the tray.

## Starting and stopping the tray

The tray starts with your graphical session. **Quit tray** in the menu closes it for this session; the daemon and anything running carry on.

```sh
systemctl --user status ctrlassist-tray
systemctl --user restart ctrlassist-tray
```
