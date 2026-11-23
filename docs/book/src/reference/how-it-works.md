# How It Works

This page is for the curious. Nothing here is needed to use CtrlAssist.

## The pieces

| Piece | Runs as | Does |
|---|---|---|
| The daemon, `ctrlassist service daemon` | You | Reads the real controllers, combines them, and writes the virtual ones. Keeps your profiles. |
| The tray, `ctrlassist service tray` | You | Shows what the daemon reports and passes on what you click. |
| The settings window, `ctrlassist-gui` | You | Shows every controller and profile, and edits profiles without writing a file. |
| The command, `ctrlassist …` | You | The same, from a terminal. |
| The hiding service, `ctrlassist-hider` | Its own system user, with no special capabilities | Makes a running arrangement's real controllers invisible to games. |

The tray, the window and the command only show what the daemon reports and pass on what you ask: close any of them and nothing that runs changes. They talk to the daemon over a socket in your runtime directory that only you can open.

## Hiding

A game that saw both the real controllers and the virtual one would get every press twice, so the real ones are kept out of its sight while an arrangement runs.

**With the hiding service**, a controller is made truly absent: nothing but the daemon can open it, and programs that already had it open, as Steam does from the moment a controller is plugged in, are cut off. When the arrangement stops the controller reappears as if just plugged in. The service is small, starts only when first asked, and serves only the person sitting at the machine. If the daemon is stopped or killed, the service restores everything it hid. A controller that is itself a virtual one, made by another program, can't be cut off that way, so the daemon grabs it, as below.

**Without it**, the daemon *grabs* each controller instead. Programs that read controllers the usual way then hear nothing from them, but a program that reads a controller some other way still sees it; Steam does this for PlayStation controllers. The tray and `ctrlassist setups` tell you which is in use.

## Latency

One thread carries all input. It sleeps until a controller has something to say, does the combining, writes the result, and sleeps again; it makes no wake-ups while nothing is pressed. The one exception is a virtual DualSense: a real one reports all the time whether or not anything is pressed, and games count on that, so a virtual one reports at least every 10 ms for as long as it exists. Measured between a real controller and the virtual one, the round trip is tens of microseconds, which is the system's own cost rather than CtrlAssist's.

That thread asks for real-time scheduling at a low priority, which keeps its latency steady when every CPU is busy. The packages allow this by letting users' services ask for real-time scheduling up to priority 10. Without it everything works the same, a little less evenly under load.

## When a controller leaves

The game's controller is CtrlAssist's virtual one, which stays. The arrangement is *held*: the missing controller reads as untouched, so nothing it was holding stays held, and the rest carries on. When the same controller returns it is hidden again before a game can see it and takes its place.

## Controller layouts

Xbox and PlayStation controllers present a standard layout through their system drivers, and CtrlAssist reads them as they are. For other controllers it uses the community controller database that SDL also uses, a snapshot of which is built in. To fix or add a layout yourself, put a line in the same format in `~/.config/ctrlassist/gamecontrollerdb.txt`; it is read when the daemon starts.

## What is kept where

| What | Where |
|---|---|
| Profiles | `~/.config/ctrlassist/profiles/` |
| Your own controller layouts | `~/.config/ctrlassist/gamecontrollerdb.txt` |
| Controllers seen before | `~/.local/state/ctrlassist/pads.toml` |
| What your own DualSense said of itself, used for the `dualsense` identity once one has been connected by USB | `~/.local/state/ctrlassist/identities/dualsense.toml` |
