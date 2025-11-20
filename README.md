# CtrlAssist

**CtrlAssist** brings "controller assist" functionality to gaming on Linux, allowing multiple physical controllers to act as a single virtual input device. This enables collaborative play and customized gamepad setups, making it easier for players of all ages and abilities—including those with limited mobility—to enjoy games together. While similar accessibility features are common on modern game consoles, CtrlAssist is an open source solution that makes this capability available for PC gaming on Linux.

## Features

- Combine physical gamepads into one virtual device
  - Primary and Assist controllers are assignable
- Customizable multiplexing of buttons and axes
  - Combining or Toggling between inputs is flexible
- Optionally hide gamepads for improved game compatibility
  - Prevent unintended detection of physical controllers

## Prerequisites
- Linux system using udev (libudev-dev)
  - and permissions to create virtual input devices
- Rust toolchain with cargo for installation
  - https://rust-lang.org/tools/install/

## Install

```sh
cargo install ctrlassist
```

## Usage

The CLI includes subcommands for locating and multiplexing gamepads.

### list

List all detected gamepads and respective their IDs:

```sh
$ ctrlassist list
Connected Gamepads:
  ID: 0 - Name: Microsoft Xbox One
  ID: 1 - Name: PS4 Controller
```

### start

Multiplex first two detected gamepads by default:

```sh
$ ctrlassist start
Controllers found and verified:
  Primary: ID: 0 - Name: Microsoft Xbox One
  Assist:  ID: 1 - Name: PS4 Controller
  Virtual: ID: 2 - Name: CtrlAssist Virtual Gamepad

Assist mode active. Press Ctrl+C to exit.
```

#### Optional: Specify Device Mapping

Manually specify Primary and Assist controllers via IDs:

```sh
$ ctrlassist start --primary 1 --assist 0
Controllers found and verified:
  Primary: ID: 1 - Name: PS4 Controller
  Assist:  ID: 0 - Name: Microsoft Xbox One
  Virtual: ID: 2 - Name: CtrlAssist Virtual Gamepad

Assist mode active. Press Ctrl+C to exit.
```

#### Optional: Hide Physical Devices

Avoiding in game conflicts by hiding physical controllers:

```sh
$ sudo ctrlassist start --hide
Controllers found and verified:
  Primary: ID: 0 - Name: Microsoft Xbox One
  Assist:  ID: 1 - Name: PS4 Controller
  Virtual: ID: 2 - Name: CtrlAssist Virtual Gamepad

Restricting device permissions (requires root)...
  Restricting: Microsoft Xbox One
    Restricted: /dev/input/event16
    Restricted: /dev/input/js0
  Restricting: PS4 Controller
    Restricted: /dev/hidraw3
    Restricted: /dev/input/event30
    Restricted: /dev/input/event256
    Restricted: /dev/input/mouse6
    Restricted: /dev/input/event31
    Restricted: /dev/input/js3

Assist mode active. Press Ctrl+C to exit.
^C
Shutdown signal received.
Restoring device permissions...
  Restored: /dev/input/event30
  Restored: /dev/input/js0
  Restored: /dev/input/js3
  Restored: /dev/input/event16
  Restored: /dev/hidraw3
  Restored: /dev/input/event256
  Restored: /dev/input/mouse6
  Restored: /dev/input/event31
```

## Limitations

- Hiding physical gamepads requires root access
  - temporarily modifies group permissions for selected devices
- Hiding is by merely matching vendor and product IDs
  - similar gamepads with matching IDs may also be hidden
- Hiding must be done before starting games or launchers
  - processes with open file handles may retain device access

## Background

Similar accessibility features:
- [Second Controller Assistance on PlayStation](https://www.playstation.com/en-us/support/hardware/second-controller-assistance/)
- [Controller Assist on Xbox and Windows](https://gameaccess.info/xbox-controller-assist-on-windows-pc/)
