![Banner](docs/artwork/banner.jpg)

**CtrlAssist** brings "controller assist" functionality to Linux gaming by allowing multiple physical controllers to operate as a single virtual input device. This enables collaborative play and customizable gamepad setups, making it easier for players of all ages and abilities to enjoy games together. While similar features exist on modern game consoles, CtrlAssist is an open source project that enhances accessibility for PC gaming, offering additional quality-of-life improvements through virtual input devices on Linux.

[![CI Pipeline](https://github.com/ruffsl/CtrlAssist/actions/workflows/ci.yml/badge.svg)](https://github.com/ruffsl/CtrlAssist/actions/workflows/ci.yml)
[![Crates.io Version](https://img.shields.io/crates/v/ctrlassist)](https://crates.io/crates/ctrlassist)
[![Crates.io Dependencies](https://img.shields.io/deps-rs/ctrlassist/latest)](https://crates.io/crates/ctrlassist/dependencies)
![Crates.io MSRV](https://img.shields.io/crates/msrv/ctrlassist)
![Crates.io Total Downloads](https://img.shields.io/crates/d/ctrlassist)
[![Crates.io License](https://img.shields.io/crates/l/ctrlassist)](https://choosealicense.com/licenses/apache-2.0)

# Features

- Combine physical controllers into one virtual gamepad
  - Controllers are assigned as either Primary or Assist
- Customizable multiplexing modes for buttons and axes
  - Logically merging or preempting events is flexible
- Hide physical controllers for improved game compatibility
  - Multiple hiding strategies for avoiding interference
- Spoof gamepad vendor for in-game layout recognition
  - Mimic either Primary or Assist controller hardware
- Rumble pass-through from virtual to physical devices
  - Forward force feedback to either or both controllers
- **System tray interface** for graphical control
  - Configure controllers and mux options from desktop
  - Start/stop muxing with live status feedback
  - Desktop notifications for status changes
  - Persistent settings across restarts

## Modes

- **Priority** (default): Assist controller overrides when active
  - Axes: Prioritize Assist when active (exceeds deadzone)
    - Buttons: Prioritize Assist when button released
    - Triggers: Prioritize largest value from either
  - Ideal for partial and asynchronous assistance
    - E.g. Assist for movement while Primary for actions
- **Average**: Blend weighted inputs from both controllers
  - Axes: Averaged when both are active (exceed deadzone)
    - Buttons: logically OR'ed between pressed controllers
    - Triggers: Averaged when both are active (exceed deadzone)
  - Ideal for cooperative input and subtle corrections
    - E.g. For counter steer/brake assist in racing games
- **Toggle**: Switch Active controller on demand
  - All inputs forwarded from currently active controller
    - Toggle Active controller via the Mode button on Assist
    - Immediately synchronizes input to current Active state
  - Ideal when fine-grain conflict-free control is needed
    - E.g. Game menu navigation or precise interventions

# Install

The following installation methods are available:

- [Cargo](#cargo) (Rust package manager)
  - Ideal for customization and unsandboxed use
  - Suitable for development and contributing
  - E.g. fork custom features and upstream fixes
- [Flatpak](#flatpak) (Linux application sandbox)
  - Ideal for easy install on SteamOS, Bazzite, etc.
  - Suitable for immutable Linux distributions
  - E.g. where installing build tools is a hassle

## Cargo

- Build dependencies
  - [libudev-dev](https://pkgs.org/search/?q=libudev-dev)
  - [pkg-config](https://pkgs.org/search/?q=pkg-config)
- Rust toolchain
  - https://rust-lang.org/tools/install/
  - configure `PATH` per Notes linked above

Add the `--force` flag to upgrade to latest version:

```sh
cargo install ctrlassist
```

## Flatpak

- Runtime dependencies
  - [Flatpak](https://flatpak.org/setup/) (likely already installed)

Download latest bundle from [releases page](https://github.com/ruffsl/ctrlassist/releases) and install:

```sh
export VERSION=v0.2.0
wget https://github.com/ruffsl/ctrlassist/releases/download/$VERSION/ctrlassist.flatpak
flatpak install --user ctrlassist.flatpak
```

Run and test via Flatpak using the application ID:

```sh
flatpak run io.github.ruffsl.ctrlassist --help
```

# Usage

Use the `--help` flag for information on each CLI subcommand:

```sh
$ ctrlassist --help
Multiplex multiple controllers into virtual gamepad

Usage: ctrlassist <COMMAND>

Commands:
  list  List all detected controllers and respective IDs
  mux   Multiplex connected controllers into virtual gamepad
  tray  Launch system tray app for graphical control
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## tray

Launch the system tray app for graphical control:

```sh
$ ctrlassist tray
CtrlAssist system tray started
Configure and control the mux from your system tray
Press Ctrl+C to exit
```

The system tray provides:
- **Controller selection** menus for Primary and Assist
- **Configuration options** for mux mode, hiding, spoofing, and rumble
- **Start/Stop buttons** with visual feedback
- **Live status indicator** in the tray icon
- **Desktop notifications** for status changes
- **Persistent settings** saved to `~/.config/ctrlassist/config.toml`

Options are greyed out while the mux is running but show current active selections.

## list

List all detected controllers and respective IDs:

```sh
$ ctrlassist list
(0) Microsoft Xbox One
(1) PS4 Controller
```

## mux

Multiplex first two detected controllers by default:

```sh
$ ctrlassist mux
Primary: (0) Microsoft Xbox One
Assist:  (1) PS4 Controller
...
Mux Active. Press Ctrl+C to exit.
```

### Primary Assist Mapping

Manually specify Primary and Assist controllers via IDs:

```sh
$ ctrlassist mux --primary 1 --assist 0
Primary: (1) PS4 Controller
Assist:  (0) Microsoft Xbox One
...
```

### Mux Mode Selection

Manually specify mode for merging controllers:

```sh
$ ctrlassist mux --mode priority
...
```

### Spoof Virtual Device

Mimic controller hardware for in-game layout recognition:

```sh
$ ctrlassist mux --spoof primary
Primary: (0) Microsoft Xbox One
Assist:  (1) PS4 Controller
Virtual: (2) Microsoft X-Box One pad (Firmware 2015)
```

> [!WARNING]
> Combining spoofing with some hiding strategies may also hide the virtual device.

### Rumble Pass-Through

Target force feedback to either or both physical controllers:

```sh
$ ctrlassist mux --rumble both
...
```

### Hide Physical Devices

There are multiple hiding strategies to avoid input conflicts:

#### Steam Input (Flatpak Compatible)

Automatically configure Steam's controller blacklist:

```sh
$ ctrlassist mux --hide steam
...
```

> [!NOTE]
> Restart Steam for blacklist to take effect; CtrlAssist reverts config on exit.

> [!WARNING]
> Combining this hiding strategy with spoofing may also hide the virtual device.

#### System Level (Root Required)

Restrict device tree permissions system-wide:

```sh
$ sudo ctrlassist mux --hide system
...
```

> [!NOTE]
> Restart game/launcher to force rediscovery; CtrlAssist reverts change on exit.

> [!IMPORTANT]
> Not possible via Flatpak sandbox for security. Use `--hide steam` instead.

# Configuration

The system tray saves settings to `~/.config/ctrlassist/config.toml`:

```toml
# Last selected controllers (by name for best-effort matching)
primary_name = "Microsoft Xbox One"
assist_name = "PS4 Controller"

# Mux configuration
mode = "Priority"
hide = "Steam"
spoof = "None"
rumble = "Both"
```

Settings are loaded on startup and automatically saved when changed. Controller names are matched by name (best-effort) if IDs change between sessions.

# Limitations

- System hiding requires root access
  - temporarily modifies group permissions for selected devices
- Hiding must be done before starting games or launchers
  - processes with open file handles may retain device access
- Reconnecting a hidden controller may revert its visibility
  - Steam hiding persists across reconnects while CtrlAssist is running
  - System hiding: custom udev rules needed for persistent permissions
- Steam hiding affects all controllers of the same make and model
  - blacklists by vendor/product ID, not individual devices
- Steam hiding requires Steam restart
  - Steam only checks controller_blacklist config on startup
- Toggle mode requires pressing all buttons and axes after startup
  - gilrs lazily initializes gamepad state used for synchronization

## Hiding Strategies

| Strategy   | Access/Compatibility         | Granularity         | Restart Required   |
|------------|-----------------------------|---------------------|--------------------|
| **Steam**  | No root, Flatpak compatible | Vendor/Product ID   | Steam only         |
| **System** | Root required, no Flatpak   | Per-device          | Game/Launcher      |

For example, use **Steam** when running CtrlAssist via Flatpak. For 2v1 scenarios, where the third player not using CtrlAssist shares the same controller make and model, use **System** to avoid hiding the third player's gamepad.

# Background

- [Controller Assist on Xbox and Windows](https://support.xbox.com/en-US/help/account-profile/accessibility/copilot)
- [Second Controller Assistance on PlayStation](https://www.playstation.com/en-us/support/hardware/second-controller-assistance/)
- [InputPlumber: Open source input router and remapper daemon for Linux](https://github.com/ShadowBlip/InputPlumber)
