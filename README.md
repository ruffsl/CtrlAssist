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

# Modes

- **Priority** (default): Assist controller overrides when active
  - Axes: Prioritize when Assist exceeds deadzone
    - Buttons: OR'ed between controllers
  - Ideal for partial and asynchronous assistance
    - E.g. Assist for movement while Primary for actions
- **Average**: Blend weighted inputs from both controllers
  - Axes: Averaged when both exceed deadzone
    - Buttons: OR'ed between controllers
  - Ideal for cooperative input and subtle corrections
    - E.g. For counter steer/brake assist in racing games
- **Toggle**: Switch active controller on demand
  - All inputs forwarded from currently active controller
    - Toggle active controller via the Mode button on Assist
  - Ideal when fine-grain conflict-free control is needed
    - E.g. Game menu navigation or precise interventions

# Prerequisites
- Linux system using udev (libudev-dev)
  - with user permissions to manage virtual devices
  - already pre-configured on most distributions
- Rust toolchain with included `cargo`
  - https://rust-lang.org/tools/install/
  - configure `PATH` per Notes linked above

# Install

```sh
cargo install ctrlassist
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
Detected controllers:
  ID: 0 - Name: Microsoft Xbox One
  ID: 1 - Name: PS4 Controller
```

## mux

Multiplex first two detected controllers by default:

```sh
$ ctrlassist mux
Connected controllers:
  Primary: ID: 0 - Name: Microsoft Xbox One
  Assist:  ID: 1 - Name: PS4 Controller
  Virtual: ID: 2 - Name: CtrlAssist Virtual Gamepad
```

### Optional: Specify Device Mapping

Manually specify Primary and Assist controllers via IDs:

```sh
$ ctrlassist mux --primary 1 --assist 0
Connected controllers:
  Primary: ID: 1 - Name: PS4 Controller
  Assist:  ID: 0 - Name: Microsoft Xbox One
  Virtual: ID: 2 - Name: CtrlAssist Virtual Gamepad
```

### Optional: Specify Mux Mode

Manually specify mode for merging controllers:

```sh
$ ctrlassist mux --mode priority
...
```

### Optional: Hide Physical Devices

Avoiding in game conflicts by hiding physical controllers:

```sh
$ sudo ctrlassist mux --hide
...

Hiding controllers... (requires root)

Assist mode active. Press Ctrl+C to exit.
^C
Shutting down.

Restoring controllers...
```

### Optional: Spoof Virtual Device

Mimic controller hardware for in-game layout recognition:

```sh
$ ctrlassist mux --spoof primary
Connected controllers:
  Primary: ID: 0 - Name: Microsoft Xbox One
  Assist:  ID: 1 - Name: PS4 Controller
  Virtual: ID: 2 - Name: Microsoft X-Box One pad (Firmware 2015)
```

# Limitations

- Hiding physical input devices requires root access
  - temporarily modifies group permissions for selected devices
- Hiding is by merely matching vendor and product IDs
  - Any controller with similar IDs may also be hidden
- Hiding must be done before starting games or launchers
  - processes with open file handles may retain device access
- Reconnecting a hidden controller reverts its visibility
  - custom udev rules should be used for persistent permissions

# Background

- [Controller Assist on Xbox and Windows](https://support.xbox.com/en-US/help/account-profile/accessibility/copilot)
- [Second Controller Assistance on PlayStation](https://www.playstation.com/en-us/support/hardware/second-controller-assistance/)
