![Banner](docs/artwork/banner.png)

**CtrlAssist** brings "controller assist" functionality to Linux gaming by allowing multiple physical controllers to operate as a single virtual input device. This enables collaborative play and customizable gamepad setups, making it easier for players of all ages and abilities to enjoy games together. While similar features exist on modern game consoles, CtrlAssist is an open source project that enhances accessibility for PC gaming, offering additional quality-of-life improvements through virtual input devices on Linux.

[![CI Pipeline](https://github.com/ruffsl/CtrlAssist/actions/workflows/ci.yml/badge.svg)](https://github.com/ruffsl/CtrlAssist/actions/workflows/ci.yml)
[![Crates.io Version](https://img.shields.io/crates/v/ctrlassist)](https://crates.io/crates/ctrlassist)
[![Crates.io Dependencies](https://img.shields.io/deps-rs/ctrlassist/latest)](https://crates.io/crates/ctrlassist/dependencies)
![Crates.io MSRV](https://img.shields.io/crates/msrv/ctrlassist)
![Crates.io Total Downloads](https://img.shields.io/crates/d/ctrlassist)
[![Crates.io License](https://img.shields.io/crates/l/ctrlassist)](https://choosealicense.com/licenses/apache-2.0)

# ✨ Features

- 🎮 Combine physical controllers into one virtual gamepad
  - Assign controllers as either Primary or Assist
- 🎛️ Customizable multiplexing modes for buttons and axes
  - Logically merging or preempting events is flexible
- 🙈 Hide physical controllers for improved game compatibility
  - Multiple hiding strategies for avoiding interference
- 🕹️ Spoof gamepad vendor for in-game layout recognition
  - Mimic either Primary or Assist controller hardware
- 🫨 Rumble pass-through from virtual to physical devices
  - Forward force feedback to either or both controllers
- 🖱️ System tray interface for graphical desktop environments
  - Configure controllers and mux options via the taskbar
  - Start/stop/alter muxing with live status notifications
  - Persistent user settings across session restarts

![System Tray Screenshot](docs/screenshots/system_tray.png)

## 🎛️ Modes

- 👑 **Priority** (default): Assist controller overrides when active
  - Axes: Prioritize Assist when active (exceeds deadzone)
    - Buttons: Prioritize Assist when button released
    - Triggers: Prioritize largest value from either
  - Ideal for partial and asynchronous assistance
    - E.g. Assist for movement while Primary for actions
- ⚖️ **Average**: Blend weighted inputs from both controllers
  - Axes: Averaged when both are active (exceed deadzone)
    - Buttons: logically OR'ed between pressed controllers
    - Triggers: Averaged when both are active (exceed deadzone)
  - Ideal for cooperative input and subtle corrections
    - E.g. For counter steer/brake assist in racing games
- 🔄 **Toggle**: Switch Active controller on demand
  - All inputs forwarded from currently active controller
    - Toggle Active controller via the Mode button on Assist
    - Immediately synchronizes input to current Active state
  - Ideal when fine-grain conflict-free control is needed
    - E.g. Game menu navigation or precise interventions

[Screencast_20251230_070245.webm](https://github.com/user-attachments/assets/40f72091-cfeb-461b-a4fb-5b4198604e9d)

# ⬇️ Install

The following installation methods are available:

- 🦀 [Cargo](#cargo) (Rust package manager)
  - Ideal for customization and unsandboxed use
  - Suitable for development and contributing
  - E.g. fork custom features and upstream fixes
- 📦 [Flatpak](#flatpak) (Linux application sandbox)
  - Ideal for easy install on SteamOS, Bazzite, etc.
  - Suitable for immutable Linux distributions
  - E.g. where installing build tools is a hassle

## 🦀 Cargo

- Build dependencies:
  - [libudev-dev](https://pkgs.org/search/?q=libudev-dev)
  - [pkg-config](https://pkgs.org/search/?q=pkg-config)
- Rust toolchain:
  - https://rust-lang.org/tools/install/
  - configure `PATH` per Notes linked above

Install or upgrade to the latest version:

```sh
cargo install ctrlassist --force
```

## 📦 Flatpak

- Runtime dependency:
  - [Flatpak](https://flatpak.org/setup/) (likely already installed)

Download latest bundle from [releases page](https://github.com/ruffsl/ctrlassist/releases) and install:

```sh
export VERSION=v0.3.0
wget https://github.com/ruffsl/ctrlassist/releases/download/$VERSION/ctrlassist.flatpak
flatpak install --user ctrlassist.flatpak
```

Run and test via Flatpak using the application ID:

```sh
flatpak run io.github.ruffsl.ctrlassist --help
```

Or launch the system tray via the installed desktop icon.

# 📖 Usage

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

## 🖱️ tray

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
- **Persistent settings** saved to disk on use

Device invariant options can be altered while the mux is running; all other options are disabled (greyed out) until the mux is stopped.

## 🧾 list

List all detected controllers and respective IDs:

```sh
$ ctrlassist list
(0) Microsoft Xbox One
(1) PS4 Controller
```

## 🔀 mux

Multiplex first two detected controllers by default:

```sh
$ ctrlassist mux
Primary: (0) Microsoft Xbox One
Assist:  (1) PS4 Controller
...
Mux Active. Press Ctrl+C to exit.
```

### 🎮 Primary Assist Mapping

Manually specify Primary and Assist controllers via IDs:

```sh
$ ctrlassist mux --primary 1 --assist 0
Primary: (1) PS4 Controller
Assist:  (0) Microsoft Xbox One
...
```

### 🎛️ Mux Mode Selection

Manually specify mode for merging controllers:

```sh
$ ctrlassist mux --mode priority
```

### 🕹️ Spoof Virtual Device

Mimic controller hardware for in-game layout recognition:

```sh
$ ctrlassist mux --spoof primary
Primary: (0) Microsoft Xbox One
Assist:  (1) PS4 Controller
Virtual: (2) Microsoft X-Box One pad (Firmware 2015)
```

> [!WARNING]
> Combining spoofing with some hiding strategies may also hide the virtual device.

### 🫨 Rumble Pass-Through

Target force feedback to either, none, or both physical controllers:

```sh
$ ctrlassist mux --rumble both
...
```

### 🙈 Hide Physical Devices

Multiple hiding strategies are available to avoid input conflicts:

| Strategy   | Access/Compatibility         | Granularity         | Restart Required   |
|------------|-----------------------------|---------------------|--------------------|
| **Steam**  | No root, Flatpak compatible | Vendor/Product ID   | Steam only         |
| **System** | Root required, no Flatpak   | Per-device          | Game/Launcher      |

Use **Steam** hiding when running CtrlAssist via Flatpak. For 2v1 scenarios, where a third player not using CtrlAssist shares the same controller make and model, use **System** to avoid hiding the third player's gamepad.

#### Steam Input

Automatically configure Steam's controller blacklist:

```sh
ctrlassist mux --hide steam
```

> [!NOTE]
> Restart Steam for blacklist to take effect; CtrlAssist reverts config on exit.

> [!WARNING]
> Combining this hiding strategy with spoofing may also hide the virtual device.

#### System Level

Restrict device tree permissions system-wide:

```sh
sudo ctrlassist mux --hide system
```

> [!NOTE]
> Restart game/launcher to force rediscovery; CtrlAssist reverts change on exit.

> [!IMPORTANT]
> Not possible via Flatpak sandbox for security. Use `--hide steam` instead.

# ⚙️ Configuration

The system tray saves settings to `$XDG_CONFIG_HOME/ctrlassist/config.toml`:

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

Settings are loaded on startup and saved when using the mux. Controllers are matched by name (best-effort) if IDs change between sessions.

# ⚠️ Limitations

- System hiding requires root access (not available in Flatpak)
  - Temporarily modifies group permissions for selected devices
- Hiding must be done before starting games or launchers
  - Processes with open file handles may retain device access
- Reconnecting a hidden controller may revert its visibility
  - Steam hiding persists across reconnects while CtrlAssist is running
  - System hiding: custom udev rules needed for persistent permissions
- Steam hiding affects all controllers of the same make and model
  - Blacklists by vendor/product ID, not individual devices
- Steam hiding requires Steam restart
  - Steam only checks controller_blacklist config on startup
- Toggle mode requires pressing all buttons and axes after startup
  - gilrs lazily initializes gamepad state used for synchronization

# ❓ FAQ

Frequently Asked Questions about the project.

### **Who is CtrlAssist for?**

CtrlAssist is designed for anyone who wants to combine multiple controllers into one, enabling collaborative play, real-time assistance, or better gamepad ergonomics. Ideal for accessibility and partial asynchronous input, i.e. offloading camera angle management, movements requiring speed and precision, or on standby backup during difficult combat encounters. CtrlAssist is especially useful for:
- Players with disabilities or motor skill challenges
- Beginners still developing muscle memory for controller gameplay
- Experienced gamers helping newcomers through challenging games
- Families with young children or older relatives who want to play together
- Racing or flight sim enthusiasts using a wheel or HOTAS (Hands On Throttle-And-Stick)
- Anyone interested in collaborative co-op for single or multiplayer games

### **Why not pass around a single controller?**

While playing "hot potato" with a gamepad may be sufficient for some scenarios, like turn-based games, divvying up level progression, menu navigation, the approach falls short for many reasons:

- Broken immersion
  - Context switching in real life pulls players out of the story
- Added friction
  - Awkward handoffs and delays interfere with real-time gameplay
- Reduced agency
  - Waiting for returned control can kill spontaneity and focus
- Deprived learning
  - No haptic feedback hinders recognizing cues like attack telegraphs
- Marginal assistance
  - Inhibited intervention may only compound unnecessary frustration

Accessibility features such as control assist address these issues by enabling simultaneous/partial input from multiple controllers, resulting in more fluid and engaging gameplay.

### **Why was CtrlAssist developed?**

CtrlAssist was first created out of personal necessity. After migrating household gaming to Linux, including the family living room, the lack of controller assist features found on older consoles like Xbox and PlayStation became clear. CtrlAssist was developed as an open source solution to make group gaming sessions on PC more inclusive and accessible for players of all ages and abilities.

Following its initial release and personal household success, as well as the broader trend of Linux adoption, CtrlAssist evolved from a simple CLI tool into a desktop-friendly utility. This category of accessibility features has significantly enhanced family gaming time, transforming passive spectators into active participants. From helping grandparents experience new immersive and interactive single player stories, to leveling age gaps across nieces and nephews in multiplayer PvPs, to rescuing friends from critical damage and finally overcoming a challenging boss, assistive players may expect as much enjoyment as primary players.

### **What games are compatible?**

CtrlAssist works with most Linux games that support standard gamepad input. Some games or launchers may require restarting after changing controller visibility or virtual device settings. Note that many games have no explicit setting for controller selection, thus the motivation for various hiding strategies to avoid input conflicts between physical and virtual devices. For best compatibility, use the appropriate hiding strategy as described above.

Even in games that natively support multiple controllers, simultaneous input from multiple devices is often not handled. Most games prioritize one controller at a time, only switching after a period of inactivity. CtrlAssist overcomes this limitation by merging inputs into a single virtual device and providing advanced multiplexing modes for input events, going beyond simple first-come, first-served behavior.

### **Which controllers are supported?**

CtrlAssist supports most standard gamepads, such as those with a conventional Xbox or PlayStation layout, including those with strong and weak force feedback (rumble) capabilities. Under the hood, the [`gilrs`](https://crates.io/crates/gilrs) crate is used for gamepad input detection and event handling, requiring that controllers [have at least 1 button and 2 axes](https://docs.rs/crate/gilrs-core/0.6.6/source/src/platform/linux/gamepad.rs#625).

However, specialized controller features such as tactile triggers, gyroscopic and accelerometer motion tracking, or more exotic force feedback waveforms are not yet supported. If you have device driver expertise and would like to contribute support for additional controller features, please consider opening a pull request!

### **Are mice or keyboards supported?**

Not directly, as CtrlAssist is focused on gamepad input multiplexing. However, it is possible to combine CtrlAssist with more advanced utilities such as [InputPlumber](https://github.com/ShadowBlip/InputPlumber) to route keyboard and mouse events to virtual gamepads and into CtrlAssist, or vice versa taking virtual gamepads from CtrlAssist to keyboard and mouse events.

Note that mouse and keyboard inputs are typically handled differently from gamepad inputs, as they are core interfaces for operating systems and display managers. Merging events from multiple mice and keyboards is often managed by the OS already, negating the need for simpler multiplexing software.

### **Is running multiple instances possible?**

Yes! For scenarios where multiple primary players would like assistance, such as true split-screen multiplayer, multiple instances of CtrlAssist can be run simultaneously. Each instance will create its own virtual gamepad device, with the tray command also creating multiple separate system tray icons and menus.

Additionally, each instance can use different hiding strategies, spoofing options, and rumble targets to suit the needs of each player. Just be mindful that selected hiding strategies do not conflict between instances, causing one virtual device to be hidden by another instance.

### **How else can CtrlAssist be used?**

Examples include:
- Dual welding one for each hand, like split Nintendo Switch Joy-Cons
- Combining a standard gamepad with an accessible Xbox Adaptive Controller

# 📚 Background

- [Controller Assist on Xbox and Windows](https://support.xbox.com/en-US/help/account-profile/accessibility/copilot)
- [Second Controller Assistance on PlayStation](https://www.playstation.com/en-us/support/hardware/second-controller-assistance/)
- [InputPlumber: Open source input router and remapper daemon for Linux](https://github.com/ShadowBlip/InputPlumber)
