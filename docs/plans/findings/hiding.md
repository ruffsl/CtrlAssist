# Finding: hiding, permissions and identification

Measured 2026-11-21 with `spikes/hiding/`: in a clean NixOS VM (`just spike-hiding-vm`), and on the development host with a DualSense over USB, a DualShock 4 over Bluetooth, Steam (client 1794536281) and SDL2's `sdl2-jstest`. Steam's view was read from its `logs/controller.txt`, not its settings screen.

**Status: verdict reached and confirmed on a real pad.** One check still needs the maintainer, identifiers across reconnects and transports, listed under [Still to run](#still-to-run).

## Verdict

**No unprivileged mechanism makes a pad absent to games. The privileged helper ships.**

The plan's rule was that a default mechanism must reach "absent" for Steam and for a native SDL game and leave nothing behind after a kill. Each unprivileged candidate fails it:

| Mechanism | Steam | SDL, default (HIDAPI) | SDL, evdev backend | Raw evdev and joydev | After a kill | After a reconnect |
|---|---|---|---|---|---|---|
| evdev grab | visible | visible | present but silent | silent | released by the kernel | gone; must be re-taken |
| Steam blacklist | absent, after a Steam restart | visible | visible | visible | stays in `config.vdf` | still applies |
| SDL ignore-devices hint | not tested | absent | absent | visible | nothing to leave (per-process environment) | still applies |
| Node mode 000, set by root | not tested | not tested | not tested | cannot open | stays until reconnect | lost: the new node has default access |
| Runtime udev rule, set by root | not tested | absent | absent | cannot open | stays until removed or reboot | still applies |

The udev rule was run on the real DualSense by the maintainer (`just spike-hiding-root`): with its event, js and hidraw nodes denied, SDL listed one pad instead of two on both backends, still did after the pad was unplugged and replugged, and listed two again once the rule was removed and the modes put back. Steam was not run against the rule; it reads the same hidraw node SDL's default backend does, so the same result is expected but unobserved.

What the rows mean for the design:

- **A grab is not hiding.** Steam and SDL read both Sony pads through hidraw (Steam logs "Controller using HIDAPI driver"; SDL's device IDs carry its HIDAPI marker), and hidraw has no grab: with both of the DualSense's event nodes grabbed, its hidraw node delivered the same 75 reports in 300 ms as before, Steam logged nothing, and SDL listed both pads. With SDL forced onto its evdev backend the pad is still listed, only silent. A grab remains useful for one thing: it silences evdev and joydev readers with no privilege and is undone by the kernel when the process dies.
- **Steam's blacklist works only for Steam, only by vendor and product, and only across a restart.** Written while Steam ran, it changed nothing, even when another pad was hot-plugged. It survived Steam's exit, and on the next start Steam logged "Hiding blacklisted Device 54c/ce6" and opened only the other pad. A uinput pad carrying the same IDs was not adopted by Steam either, as the legacy README warned. Games Steam doesn't launch are unaffected.
- **SDL's hint works only for SDL, only by vendor and product, and only at launch.** It is an environment variable of the game's process, so a daemon can't apply it to a game that is already running, and it would also hide a virtual pad that copies those IDs.
- **Only node permissions cover every consumer at once,** and they need root.

## The helper

**It can't be a permission rule alone.** The daemon and the games are the same user, so whatever denies the games denies the daemon. The VM confirmed the way out: a reader that opened the node *before* it was set to mode 000 kept receiving (149 events over the window). So access has to arrive as an open file descriptor rather than as a permission:

1. The daemon asks the helper to hide a pad.
2. The helper installs a runtime udev rule for that pad (event, js and hidraw nodes: no access, no `uaccess` tag), opens the nodes itself, and passes the descriptors to the daemon over its socket.
3. On reconnect the rule already covers the new nodes, so there is no window in which a game sees the pad; the helper passes fresh descriptors.
4. When the daemon's connection closes, for any reason including a kill, the helper removes the rule and restores the nodes. That ties cleanup to something the kernel reports, which is the crash rule the plan asked for.

Details the VM settled:

- **A rule is needed, not a chmod.** A chmod is lost when the pad reconnects.
- **The rule file must sort after systemd's `70-uaccess.rules` and before `73-seat-late.rules`.** Numbered 59, it hid a connected pad but not a reconnected one, because rule 70 re-added the tag afterward. Numbered 72, it held across a reconnect.
- **Removing the rule does not put the mode back.** After removal and a re-trigger the ACL returned but the mode stayed 000, so the helper must restore the mode it found.
- **Rules under `/run/udev/rules.d` vanish at reboot,** which bounds the worst case if the helper itself dies.

The smallest interface is two requests, hide and restore, each naming a pad, with hide returning descriptors. Packaging cost: one more binary, a system socket unit, and a policy for who may connect (the active seat user, as TisVlay's socket ACLs do). Channels that can't install it fall back to a grab plus the Steam blacklist, with the gaps in the table above.

## Permissions on a clean machine

A seat user with no extra groups, on NixOS with only systemd's own rules:

| Node | Mode, owner | Seat user can open |
|---|---|---|
| `/dev/uinput` | 600 root:root | no |
| `/dev/uhid` | 600 root:root | no |
| a pad's event node | 660 root:input, plus a `uaccess` ACL | yes |
| a pad's js node | 664 root:input, plus a `uaccess` ACL | yes |
| a pad's hidraw node | 600 root:root | no |

So out of the box a user can read pads but can't create a virtual pad of either kind. The development host differs in every "no" row: Steam's and Sunshine's rules open uinput, uhid and the Sony pads' hidraw nodes, and the maintainer's account is also in the `input` group.

The packages must therefore install a udev rule granting the seat user `/dev/uinput`, and `/dev/uhid` too if virtual pads are made that way. hidraw access is needed only if the daemon reads pads through hidraw, which is the identity spike's question.

## Identification

From the two pads connected:

| | DualSense, USB | DualShock 4, Bluetooth |
|---|---|---|
| `uniq` (and `HID_UNIQ`) | `0c:27:56:4f:bd:14` | `1c:96:5a:a1:35:da` |
| `phys` on the input device | empty | empty |
| `ID_SERIAL` | the product name, no serial | absent |
| `ID_PATH` | the USB port | absent |
| vendor, product | 054c, 0ce6 | 054c, 05c4 |

- **`uniq` is the pad's own address on both transports here,** and Steam keys its per-pad configuration by the same value (`configset_DS0c27564fbd14.vdf`, `configset_1c965aa135da.vdf`). It is the candidate for the stable hardware ID.
- **`ID_SERIAL` and `ID_PATH` are not identifiers:** one is the model name, the other is the port, and neither exists over Bluetooth.
- **A uinput pad has an empty `uniq`,** and Steam invents a serial for it from its IDs. The daemon's own virtual pads need a marker of their own to be recognizable as sources to exclude; a `uniq` value is the obvious place.
- **A Bluetooth pad is a uhid device.** The DualShock 4's parent in sysfs is `/devices/virtual/misc/uhid`: the Bluetooth daemon creates it through the same interface the identity spike will try. So "under uhid" does not mean "virtual", and the reverse check (is this one of ours?) has to use the marker.

Not yet known: whether `uniq` is the same for one pad over USB and over Bluetooth, whether it survives a reconnect unchanged, and what an Xbox pad reports.

## Other observations

- **Steam adopts any uinput pad at once** as an SDL device, reserves an XInput slot for it, and closes it when it disappears ("Controller device closed after hid_read failure").
- **`uhid-virt` needs libclang to build** (its FFI layer runs bindgen). The spike writes the create event by hand instead, about thirty lines. That is a cost to weigh in the identity spike.
- **The spike's fixtures work as test infrastructure.** Pads made through uinput and uhid, a probe that counts what one reader sees, and a NixOS VM that runs them as a flake check took seconds per run. The device crate's tests can be built the same way.

## InputPlumber

Not tested: it isn't installed on the development host, and the first release is verified on NixOS only. From the survey, it hides pads the way the helper above would (root, udev rules) and would contend for the same pads. Cooperation would mean a second code path that can't be exercised here. Recommendation: stay independent for the first release, document that the two shouldn't manage the same pad, and revisit when someone on Bazzite can test.

## Still to run

Needs the maintainer; results are logged under `spikes/hiding/target/`.

**Identifiers:** run `just spike-hiding-ids "<label>"` in each of these states: the DualSense on USB after a replug; the DualSense over Bluetooth; an Xbox pad on USB; the Xbox pad over Bluetooth.

## What this changes in the plan

- The helper moves from provisional to needed, with the descriptor-passing shape above.
- The device crate reads pads from descriptors it is handed, not from paths it opens, when hiding is on.
- `uniq` is the working hardware ID, pending the identifier check.
- The daemon marks its virtual pads so it can exclude them as sources.
- InputPlumber cooperation is dropped from the first release.
