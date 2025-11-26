![Banner](docs/artwork/banner.jpg)

**CtrlAssist** brings "controller assist" functionality to Linux gaming by allowing multiple physical controllers to operate as a single virtual input device. This enables collaborative play and customizable gamepad setups, making it easier for players of all ages and abilities to enjoy games together. While similar features exist on modern game consoles, CtrlAssist is an open source project that enhances accessibility for PC gaming, offering additional quality-of-life improvements through virtual input devices on Linux.

# Features

- Combine physical controllers into one virtual gamepad
  - Controllers are assigned as either Primary or Assist
- Customizable multiplexing of buttons and axes
  - Logically merging or preempting events is flexible
- Hide physical controllers for improved game compatibility
  - Avoid controller interference from conflicting inputs
- Spoof gamepad vendor for in-game layout recognition
  - Mimic either Primary or Assist controller hardware

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

### Optional: Hide Physical Devices

Avoiding in game conflicts by hiding physical controllers:

```sh
$ sudo ctrlassist mux --hide
Connected controllers:
  Primary: ID: 0 - Name: Microsoft Xbox One
  Assist:  ID: 1 - Name: PS4 Controller
  Virtual: ID: 2 - Name: CtrlAssist Virtual Gamepad

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

# Todo

- [ ] Fix Primary axis value restoration for Preempt mode
- [ ] Add back mux mode for toggling between Primary and Assist
- [ ] Fix spoofing for play stations controllers (i.e. DualShock)
- [ ] Add config file support for expressive multiplexing settings
- [ ] Add GUI for easier configuration and usage
- [ ] Add more robust error handling for dynamic device connectivity
- [ ] Register project on crates.io for easier installation
- [ ] Package project for popular Linux distributions
- [ ] Write more comprehensive documentation and usage examples
- [ ] Gather user feedback for future improvements and features
- [ ] Optimize performance for low-latency input handling
 