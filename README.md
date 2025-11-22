# CtrlAssist

**CtrlAssist** brings "controller assist" functionality to gaming on Linux, allowing multiple physical controllers to act as a single virtual input device. This enables collaborative play and customized gamepad setups, making it easier for players of all ages and abilities to enjoy games together. While similar accessibility features are common on modern game consoles, CtrlAssist is an open source solution that makes this capability available for PC gaming on Linux.

## Features

- Combine physical controllers into one virtual gamepad
  - Controllers are assigned as either Primary or Assist
- Customizable multiplexing of buttons and axes
  - Logically merging or preempting events is flexible
- Hide physical controllers for improved game compatibility
  - Avoid controller interference from conflicting inputs

## Prerequisites
- Linux system using udev (libudev-dev)
  - with user permissions to manage virtual devices
  - already pre-configured on most distributions
- Rust toolchain with included `cargo`
  - https://rust-lang.org/tools/install/
  - configure `PATH` per Notes linked above

## Install

```sh
cargo install ctrlassist
```

## Usage

The CLI includes subcommands for locating and multiplexing controllers.

### list

List all detected controllers and respective IDs:

```sh
$ ctrlassist list
Detected controllers:
  ID: 0 - Name: Microsoft Xbox One
  ID: 1 - Name: PS4 Controller
```

### mux

Multiplex first two detected controllers by default:

```sh
$ ctrlassist mux
Connected controllers:
  Primary: ID: 0 - Name: Microsoft Xbox One
  Assist:  ID: 1 - Name: PS4 Controller
  Virtual: ID: 2 - Name: CtrlAssist Virtual Gamepad

Assist mode active. Press Ctrl+C to exit.
```

#### Optional: Specify Device Mapping

Manually specify Primary and Assist controllers via IDs:

```sh
$ ctrlassist mux --primary 1 --assist 0
Connected controllers:
  Primary: ID: 1 - Name: PS4 Controller
  Assist:  ID: 0 - Name: Microsoft Xbox One
  Virtual: ID: 2 - Name: CtrlAssist Virtual Gamepad

Assist mode active. Press Ctrl+C to exit.
```

#### Optional: Hide Physical Devices

Avoiding in game conflicts by hiding physical controllers:

```sh
$ sudo ctrlassist mux --hide
Connected controllers:
  Primary: ID: 0 - Name: Microsoft Xbox One
  Assist:  ID: 1 - Name: PS4 Controller
  Virtual: ID: 2 - Name: CtrlAssist Virtual Gamepad

Hiding controllers... (requires root)
  Hiding: Microsoft Xbox One
    Hidden: /dev/input/event16
    Hidden: /dev/input/js0
  Hiding: PS4 Controller
    Hidden: /dev/hidraw16
    Hidden: /dev/input/event17
    Hidden: /dev/input/event18
    Hidden: /dev/input/event19
    Hidden: /dev/input/js2
    Hidden: /dev/input/mouse0

Assist mode active. Press Ctrl+C to exit.
^C
Shutdown signal received.

Restoring controllers...
  Restored: /dev/input/event17
  Restored: /dev/hidraw16
  Restored: /dev/input/js0
  Restored: /dev/input/js2
  Restored: /dev/input/event16
  Restored: /dev/input/mouse0
  Restored: /dev/input/event19
  Restored: /dev/input/event18
```

## Limitations

- Hiding physical input devices requires root access
  - temporarily modifies group permissions for selected devices
- Hiding is by merely matching vendor and product IDs
  - Any controller with similar IDs may also be hidden
- Hiding must be done before starting games or launchers
  - processes with open file handles may retain device access
- Reconnecting a hidden controller reverts its visibility
  - custom udev rules should be used for persistent permissions

## Background

- [Controller Assist on Xbox and Windows](https://support.xbox.com/en-US/help/account-profile/accessibility/copilot)
- [Second Controller Assistance on PlayStation](https://www.playstation.com/en-us/support/hardware/second-controller-assistance/)
