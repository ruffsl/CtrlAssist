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
  - Avoid controller interference from conflicting inputs
- Spoof gamepad vendor for in-game layout recognition
  - Mimic either Primary or Assist controller hardware
- Rumble pass-through from virtual to physical devices
  - Forward force feedback to either or both controllers

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
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

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

### Rumble Pass-Through

Target force feedback to either or both physical controllers:

```sh
$ ctrlassist mux --rumble both
...
```

### Hide Physical Devices

Avoiding in game conflicts by hiding physical controllers:

```sh
$ sudo ctrlassist mux --hide
...
```

> [!IMPORTANT]  
> Running ctrlassist as root is not possible in Flatpak due to sandboxing.
> Continue reading the [Workarounds](#workarounds) section for alternatives.

# Limitations

- Hiding physical input devices requires root access
  - temporarily modifies group permissions for selected devices
- Hiding must be done before starting games or launchers
  - processes with open file handles may retain device access
- Reconnecting a hidden controller reverts its visibility
  - custom udev rules should be used for persistent permissions
- Toggle mode requires pressing all buttons and axes after startup
  - gilrs lazily initializes gamepad state used for synchronization

# Workarounds

## Hiding Physical Devices using Flatpak

Because Flatpak sandboxing prevents use of elevated privileges, using CtrlAssist to auto hide physical controllers system-wide is not as viable. However, for game launchers that support controller blacklisting, the same goal of avoiding input conflicts is still achievable.

### Steam Input

Steam Input can be configured to ignore controllers by matching vendor and product IDs. First, identify the IDs of the physical controllers to be hidden using [`lsusb`](https://pkgs.org/search/?q=usbutils): 

```sh
$ lsusb
Bus 001 Device 002: ID 045e:02dd Microsoft Corp. Xbox One Controller (Firmware 2015)
Bus 001 Device 010: ID 054c:05c4 Sony Corp. DualShock 4 [CUH-ZCT1x]
...
```

Reformat IDs using `/` and `,` to edit and save `~/.local/share/Steam/config/config.vdf` like so (ignore `+`):

```diff
"InstallConfigStore"
{
+	"controller_blacklist"	"045e/02dd,054c/05c4"
	"Software"
```

Run CtrlAssist without the `--hide` flag nor spoofing either primary or assist controllers, as CtrlAssist spoofs the former by default:

```sh
flatpak run io.github.ruffsl.ctrlassist mux --spoof none
```

Finally, restart Steam for the changes to take effect. Note that all similar controllers matching the blacklisted vendor/product IDs will then be ignored by Steam Input. This could be an issue for something like a 2v1 scenario where the third player not using CtrlAssist shares the same controller make and model.

# Background

- [Controller Assist on Xbox and Windows](https://support.xbox.com/en-US/help/account-profile/accessibility/copilot)
- [Second Controller Assistance on PlayStation](https://www.playstation.com/en-us/support/hardware/second-controller-assistance/)
- [InputPlumber: Open source input router and remapper daemon for Linux](https://github.com/ShadowBlip/InputPlumber)
