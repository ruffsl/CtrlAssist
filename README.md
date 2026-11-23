![Banner](docs/artwork/banner.png)

**CtrlAssist** brings "controller assist" to Linux gaming: several physical controllers act as one virtual controller, so a second person can help someone play, or one controller acts as several. It works below the game, so the game sees one ordinary controller and needs no support for any of this. It makes games easier to share for players of all ages and abilities: a parent taking the hard jumps for a child, a friend steering while you handle the buttons, or a player with limited mobility sharing a game's controls with a helper.

> [!NOTE]
> This branch is 0.5.0, a clean rewrite that hasn't been released yet. The last release, 0.4.0, is at the [`v0.4.0`](https://github.com/CtrlAssist/CtrlAssist/tree/v0.4.0) tag, with its own README and install instructions. Nothing from 0.4.0's configuration carries over.

## Features

- **Two controllers as one,** in three ways:
  - **Priority:** the primary plays, and whatever the assist touches, the assist takes over for as long as it is touched.
  - **Average:** the sticks and triggers are blended between the two.
  - **Toggle:** one plays at a time, handed over by a button or from the tray.
- **One controller as several,** one at a time (unicast) or all at once (multicast).
- **Arrangements of your own,** described in a short profile file, for anything the presets above don't cover, such as one helper assisting two players in turn.
- **Rumble** from the game reaches the real controllers, and you choose which.
- **The real controllers are hidden** from games while an arrangement runs, so nothing sees a press twice, and they come back when it stops. With the small hiding service installed that includes Steam; without it they are grabbed, which most games respect.
- **The virtual controller can present itself** as a plain gamepad, an Xbox Series controller, or a DualSense, whose lightbar, player lights and trigger effects reach a real DualSense behind it.
- **A controller that disconnects** leaves its arrangement held, not broken; it takes its place again when it returns.
- **A system tray** to start and stop arrangements, pick who plays which part, and see what is running, with notifications when something needs you.
- **A settings window** that shows every controller and profile and edits profiles without writing a file, and can be driven by keys alone, from a gamepad or a remote under Plasma Bigscreen, in a layout for the couch.
- **A command,** `ctrlassist`, for all of the same from a terminal, with shell completions.
- **Low latency:** one thread carries all input, with real-time scheduling where the system allows it; the round trip from a real controller to the virtual one is tens of microseconds.

## How it fits together

| Piece | Runs as | Does |
|---|---|---|
| `ctrlassist service daemon` | you | Reads the real controllers, combines them, writes the virtual ones, and keeps your profiles. |
| `ctrlassist service tray` | you | The system tray. |
| `ctrlassist-gui` | you | The settings window. |
| `ctrlassist …` | you | The command line. |
| `ctrlassist-hider` | root, started on demand | Hides a running arrangement's real controllers from games, and nothing else. |

The tray, the window and the command only show what the daemon reports and pass on what you ask, so closing any of them changes nothing that runs. The design is in [docs/architecture.md](docs/architecture.md).

## Installing

On NixOS, add the flake and set `services.ctrlassist.enable = true`. For Debian, Ubuntu, Fedora and Arch, each release will carry a `.deb`, an `.rpm` and an Arch package for x86_64 and arm64; until 0.5.0 is released they are built from source with `just package-all`. [Installation](docs/book/src/getting-started/installation.md) has the details, and [Requirements](docs/book/src/reference/requirements.md) what it runs on: Linux with systemd, and for the settings window a Wayland session. It is developed and used on NixOS with KDE Plasma.

## Documentation

The user guide is an mdBook under [`docs/book`](docs/book/src/SUMMARY.md); `just docs-serve` builds and serves it.

- New here? Start with [Installation](docs/book/src/getting-started/installation.md), then the [Quick Start](docs/book/src/getting-started/quick-start.md).
- Everyday use: [The Tray](docs/book/src/guide/tray.md), [Profiles](docs/book/src/guide/profiles.md) and the [Command Line](docs/book/src/guide/cli.md).
- Something wrong? See [Troubleshooting](docs/book/src/operations/troubleshooting.md).
- Want to help? See the [Contributing Guide](.github/CONTRIBUTING.md); how the workspace builds and is tested is in [docs/build.md](docs/build.md) and [docs/testing.md](docs/testing.md).

## Acknowledgments

The third-party notices ship with the packages. Notable among what CtrlAssist builds on:

- [Slint](https://slint.dev), the declarative toolkit of the settings window.
- [Zenoh](https://zenoh.io), the pub/sub that carries everything between the daemon and its clients over a Unix socket.
- [ksni](https://github.com/iovxw/ksni), the StatusNotifierItem library behind the tray.
- [evdev](https://github.com/cmr/evdev), for reading controllers and making virtual ones.
- [SDL_GameControllerDB](https://github.com/mdqinc/SDL_GameControllerDB), the community controller database, a snapshot of which is built in.
- [TisVlay](https://github.com/TisVlay/TisVlay), whose tooling, settings window and conventions this rewrite is built on.

## License

[Apache-2.0](LICENSE).
