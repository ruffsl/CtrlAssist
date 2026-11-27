<!-- ANCHOR: overview -->
![Banner](docs/artwork/banner.png)

**CtrlAssist** brings "controller assist" to Linux gaming, as Xbox's Controller Assist and PlayStation's Second Controller Assistance do on consoles: several controllers act as one, so a second person can help someone play. It works below the game, which sees one ordinary controller and needs no support for any of it. It makes games easier to share for players of all ages and abilities: a parent taking the hard jumps for a child, a friend steering while you handle the buttons, a player with limited mobility sharing a game's controls with a helper.

[![CI Pipeline](https://github.com/CtrlAssist/CtrlAssist/actions/workflows/ci.yml/badge.svg)](https://github.com/CtrlAssist/CtrlAssist/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/CtrlAssist/CtrlAssist/branch/main/graph/badge.svg)](https://codecov.io/gh/CtrlAssist/CtrlAssist)
[![Documentation](https://readthedocs.org/projects/ctrlassist/badge/?version=latest)](https://ctrlassist.readthedocs.io/)
[![Latest release](https://img.shields.io/github/v/release/CtrlAssist/CtrlAssist)](https://github.com/CtrlAssist/CtrlAssist/releases)
[![License](https://img.shields.io/github/license/CtrlAssist/CtrlAssist)](https://github.com/CtrlAssist/CtrlAssist/blob/main/LICENSE)

## ✨ Features

- 🎮 Two controllers as one virtual controller
  - A Primary for the main player and an Assist for the helper
  - Swap who is who, or hand over, without the game noticing
- 🎛️ Modes for how the two combine
  - 👑 Priority: the assist takes over whatever it touches
  - ⚖️ Average blends the two, 🔃 Toggle takes turns
- 🔁 One controller as several
  - 🔂 Unicast plays one virtual controller at a time
  - 📢 Multicast plays them all at once
- 🙈 Real controllers hidden while it runs
  - A game never sees a press twice, Steam included
  - They come back when the profile stops
- 🕹️ A virtual controller games recognize
  - Appears as a plain gamepad, an Xbox or a DualSense
  - Xbox or PlayStation button prompts, where a game has them
- 🫨 Feedback reaches the real controllers
  - Rumble to the primary, the assist, or both
  - A DualSense's lightbar, player lights and trigger effects
- 🧭 Motion and touchpad pass-through
  - Gyro and touchpad of a DualSense or DualShock 4
  - The assist can take the aim while holding a button
- 🔌 Disconnects are held, not broken
  - The other player carries on with the same virtual controller
  - A returning controller takes its place again
- 🖱️ Graphical user interfaces
  - A window for controllers, profiles and settings
  - A system tray for quick start, stop and hand-over
- 🛋️ Couch mode for the big screen
  - Drive the window from a TV remote or a gamepad, no mouse needed
  - Scaled-up 10-foot layout, automatic under Plasma Bigscreen
- ⌨️ Scriptable
  - A `ctrlassist` command for everything, with shell completions
  - Profiles as short files, for arrangements of your own
- ⚡ Low latency
  - One thread carries all input, real-time where the system allows
  - Tens of microseconds from a real controller to the virtual one

## 📸 Screenshots

The window and the tray follow your desktop's theme and accent color.

| **Pads** — every controller and what has it | **Pads** — in the light theme |
|:---:|:---:|
| ![Pads page showing a DualSense that the running couch profile has hidden](docs/screenshots/pads_pane.png) | ![Pads page in the light theme, on the assist's controller](docs/screenshots/pads_pane_light.png) |
| **Profiles** — who plays which part | **Has control** — hand over a Toggle |
| ![Profiles page on a running profile, with its Stop button and its two controllers](docs/screenshots/profiles_pane.png) | ![Profiles page on a running Toggle profile, with who has control](docs/screenshots/profiles_pane_running.png) |
| **Virtual controller** — what games see | **Settings** — notifications and couch mode |
| ![Pads page on the virtual DualSense a running profile makes](docs/screenshots/pads_pane_virtual.png) | ![Settings page with a switch for each kind of notification](docs/screenshots/settings_pane.png) |
| **Tray tip** — what runs, at a glance | **Tray menu** — start, stop and change a profile |
| ![Tray icon with a green mark for a running profile, and its tip listing the default profile running and the couch profile ready](docs/screenshots/system_tray_hover.png) | ![Tray menu open on the running default profile, with its controllers, mode, rumble, gyro and visibility](docs/screenshots/system_tray_menu.png) |
| **About** — every part and its release | |
| ![About page listing the daemon, the hiding service, the tray, the window and the command](docs/screenshots/about_pane.png) | |

## 🏛️ Architecture

CtrlAssist is a few cooperating parts:

```mermaid
graph LR

subgraph User
    Gui["ctrlassist-gui<br/><i>settings window</i>"]
    Cli["ctrlassist<br/><i>CLI</i>"]
    Tray["ctrlassist tray<br/><i>system tray</i>"]
    Daemon["ctrlassist daemon<br/><i>user daemon</i>"]
end

subgraph System
    Hider["ctrlassist-hider<br/><i>hiding service</i>"]
end

subgraph Controllers
    Real["Real controllers"]
    Virtual["Virtual controller"]
end

Game["Game"]

Gui -->|socket| Daemon
Cli -->|socket| Daemon
Tray -->|socket| Daemon
Daemon -->|socket| Hider
Real -.->|input| Daemon
Daemon -.->|input| Virtual
Virtual -.-> Game
Hider -.->|hides| Real
```

| Component | Description |
|-----------|-------------|
| `ctrlassist` | CLI for everything the window and the tray do |
| `ctrlassist service daemon` | Reads the real controllers, combines them, makes the virtual ones, keeps the profiles |
| `ctrlassist service tray` | System tray for quick access and desktop notifications |
| `ctrlassist-gui` | Controllers, profile editor, settings |
| `ctrlassist-hider` | Hides a running profile's real controllers from games |

The daemon and the tray ship as `ctrlassist` subcommands and run as you; `ctrlassist-gui` and `ctrlassist-hider` are binaries of their own. The hiding service is the one system service: it runs as a user of its own, with no special capabilities, and is started on demand.

<!-- ANCHOR_END: overview -->

> [!NOTE]
> This branch is 0.5.0, a clean rewrite that hasn't been released yet. The last release, 0.4.0, is at the [`v0.4.0`](https://github.com/CtrlAssist/CtrlAssist/tree/v0.4.0) tag, with its own README and install instructions. Nothing from 0.4.0's configuration carries over.

## 📖 Documentation

Rendered docs available at [ctrlassist.readthedocs.io](https://ctrlassist.readthedocs.io/). Relative source links below:

- 🧭 New here? Start with [Installation](docs/book/src/getting-started/installation.md), then [Quick Start](docs/book/src/getting-started/quick-start.md).
- 🎛️ Setting things up? See [GUI & Tray](docs/book/src/guide/gui-tray.md) and [Profiles](docs/book/src/guide/profiles.md).
- 🩺 Something wrong? See [Troubleshooting](docs/book/src/operations/troubleshooting.md) and the [FAQ](docs/book/src/reference/faq.md).
- ⌨️ Scripting? See the [Command Line](docs/book/src/guide/cli.md) and [Configuration Files](docs/book/src/guide/configuration.md).
- 🏗️ Curious how it works? See [How It Works](docs/book/src/reference/how-it-works.md), and the design in [docs/architecture.md](docs/architecture.md).
- 🤝 Want to help? See the [Contributing Guide](.github/CONTRIBUTING.md).

## 🙏 Acknowledgments

The third-party notices ship with the packages. Notable among what CtrlAssist builds on:

- [Slint](https://slint.dev), the declarative toolkit of the settings window.
- [Zenoh](https://zenoh.io), the pub/sub that carries everything between the daemon and its clients over a Unix socket.
- [ksni](https://github.com/iovxw/ksni), the StatusNotifierItem library behind the tray.
- [evdev](https://github.com/cmr/evdev), for reading controllers and making virtual ones.
- [SDL_GameControllerDB](https://github.com/mdqinc/SDL_GameControllerDB), the community controller database, a snapshot of which is built in.
- [TisVlay](https://github.com/TisVlay/TisVlay), whose tooling, settings window and conventions this rewrite is built on.
