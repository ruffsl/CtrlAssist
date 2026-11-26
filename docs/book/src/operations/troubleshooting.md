# Troubleshooting

## Where to look first

The window's **About** page says which parts of CtrlAssist are running and the release each runs, and its **Diagnostics** button copies a report to paste into a bug report. The dots at the foot of the window's sidebar say the same at a glance.

From a terminal, which is where the logs are:

```sh
ctrlassist status          # is the daemon there, and which release
ctrlassist pads            # which controllers it sees
ctrlassist setups          # what is running, and what each is waiting for
journalctl --user -u ctrlassist-daemon -e
journalctl --user -u ctrlassist-tray -e
journalctl -u ctrlassist-hider -e
```

## "no daemon is running"

The daemon is a service of your user, started at login.

```sh
systemctl --user status ctrlassist-daemon
systemctl --user enable --now ctrlassist-daemon
```

Right after installing, log out and back in once.

## A controller isn't listed

`ctrlassist pads` lists what the system reports as a gamepad. If yours is missing, check that the system sees it at all (`ls /dev/input/js*`). A controller hidden by another running arrangement is still listed, with that arrangement's name. A Steam Controller isn't listed while Steam has it ([Steam Controller](../guide/steam-controller.md#steam)), nor on a kernel too old to know it ([Requirements](../reference/requirements.md)).

## The game sees the real controllers as well as the virtual one

Look at the tray menu's **Hiding:** line, just above **Settings**, or the daemon's log from when it started.

- **Hiding: service** means controllers are truly hidden while an arrangement runs. If the game still shows one, it was probably started before the arrangement; restart the game.
- **Hiding: grab** means the hiding service hasn't been reachable, so controllers are only grabbed. Most games then see just the virtual controller, but a program that reads controllers its own way still sees the real ones. Steam does this for PlayStation controllers. Check `systemctl status ctrlassist-hider.socket`. The daemon looks for the service again each time an arrangement starts, so once the service is there, stop and start the arrangement. While controllers are only grabbed, CtrlAssist also asks Steam to leave their models alone, by adding them to Steam's controller blacklist for as long as the arrangement runs; Steam takes that up only when it next starts, so restart Steam after starting the arrangement.
- **Hiding: looking for the service…** means the daemon has only just started and hasn't heard from the hiding service yet. It changes to `service` or `grab` as soon as it has; an arrangement started meanwhile waits for that answer.

## The game sees two controllers with a Steam Controller

The hiding service doesn't take a Steam Controller, so Steam, and games that read it through SDL's own driver for it, still see it beside the virtual controller. [Steam Controller](../guide/steam-controller.md#grabbed-not-hidden) says why and what little can be done.

## A Steam Controller's raw interface "isn't yours to open"

A profile with a Steam Controller fails to start, or is held, saying so. CtrlAssist reads the controller through its raw interface, which CtrlAssist's udev rule gives to the person at the machine. The packages and the NixOS module install the rule and apply it to controllers already connected, so after installing either, this means one of these:

- **You aren't the person at the machine:** logged in over SSH, or another user's session is the one in front. The rule serves only the session in front of the seat.
- **CtrlAssist was installed some other way,** without its udev rule. Install the rule, then unplug the controller and plug it in again.

<details><summary id="raw-interface-from-a-terminal">From a terminal</summary>

```sh
ls -l /dev/hidraw*     # the controller's raw interface has a + after its mode: yours to open
loginctl               # is your session the one in front of seat0?
```

</details>

If it says the raw interface "can't be opened" or "can't be read" instead, unplug the controller and plug it in again; the daemon's log has the details.

## "no virtual pad can be made"

Virtual controllers are made through `/dev/uinput`, which the packaged udev rule gives to whoever is at the machine.

```sh
ls -l /dev/uinput
ctrlassist identities      # the AVAILABLE column
```

If it isn't writable for you: log out and back in, and check the `uinput` module is loaded (`lsmod | grep uinput`). Over SSH you are not "at the machine", and the rule doesn't apply.

## An arrangement is "held"

A controller it needs isn't connected. `ctrlassist setups` says which. Reconnect it and the arrangement resumes; nothing needs restarting.

## The tray icon is missing

On GNOME, install and enable the AppIndicator extension. The tray is still running without it, for notifications: `systemctl --user status ctrlassist-tray`.

## The settings window doesn't open

It needs a Wayland session, and doesn't open in an X11 one. Started from a terminal, `ctrlassist-gui` says why it couldn't open.

## Input feels delayed when the machine is busy

The daemon asks for real-time scheduling for the one thread that carries input, and runs without it if refused. The tray menu's **Real-time:** line and the daemon's log say which.

```sh
journalctl --user -u ctrlassist-daemon | grep -i real-time
```

If it was refused, the limit that allows it hasn't reached your session: log out and back in after installing. On NixOS, check `services.ctrlassist.realtime` is on.

## "runs release …, which this command can't talk to"

The daemon and the command (or the tray) are from different releases, usually just after an upgrade while the old daemon is still running. Restart it:

```sh
systemctl --user restart ctrlassist-daemon ctrlassist-tray
```

## After the daemon stops

Stopping the daemon, or it crashing, removes every virtual controller and gives the real ones back, hidden or not. A game will see its controller disconnect. Arrangements with `managed` come back when the daemon does.
