# A pad with a keyboard's key on its own node is grabbed, not hidden

The hiding rule gives the service's group a pad's `uevent` file, through which the service has its driver re-bound, only for an event node that udev marks as a joystick and as nothing a keystroke or a pointer could come from: one with `ID_INPUT_KEYBOARD`, `ID_INPUT_MOUSE`, `ID_INPUT_TOUCHPAD`, `ID_INPUT_TABLET` or `ID_INPUT_POINTINGSTICK` set is passed over, and one with `ID_INPUT_KEY` set is taken only where every key its input device has is at code 128 or above (`packaging/udev/72-ctrlassist-hide.rules`). Below 128 are every typing, editing, arrow and modifier key, Enter, Esc and F1 to F12. The service makes the same test before it hides anything (`is_only_a_pad` and `keys_all_high` in `crates/ctrlassist-hider/src/pad.rs`), reading the same sysfs file, `capabilities/key`, and refuses anything else as not a pad, so the daemon grabs it instead. A unit test holds the rule's patterns and the service's reading to the same answer for every bitmap the kernel writes, in words of 64 bits and of 32. Separately, the service refuses a pad whose HID device or USB interface has a keyboard or a pointer among its other inputs, which a re-bind would cut off too.

udev sets `ID_INPUT_KEY` for any one key code in 0 to 255, 0x160 to 0x21f or 0x230 to 0x2bf (systemd 262, `src/udev/udev-builtin-input_id.c`), `ID_INPUT_KEYBOARD` only when all of codes 1 to 31 are there, and drops `ID_INPUT_JOYSTICK` from a joystick with four or more of ten named keyboard keys.

**Hidden as any pad is.** A pad whose only keys are at 128 or above:

- An Xbox Series pad, its share button being `KEY_RECORD` (167): on USB by the xpad driver, which reports it for the Microsoft pad (`045e:0b12`) and some twenty others it marks `MAP_SHARE_BUTTON` (Linux 6.18); over Bluetooth by hid-microsoft (`045e:0b13`), whose report descriptor has the consumer page's Record in the game pad's own report; and by the out-of-tree xone driver. The hider's VM check makes one on Bluetooth through uhid with that descriptor, has hid-microsoft bind it, and hides it and shows it again, with a twin plugged in while it is hidden.
- An 8BitDo N30 Pro 2 over Bluetooth, with a menu key (`KEY_MENU`, 139), by its descriptor as others have recorded it; not tried.

PlayStation, Switch and Steam pads have no keys at all and were hidden before.

**Still refused, and grabbed:**

- A Stadia pad, by its descriptor as others have recorded it: its volume keys (`KEY_VOLUMEDOWN` and `KEY_VOLUMEUP`, 114 and 115) are below the line. Accepted, so that no key a keyboard types with is ever hidden.
- An Xbox Elite Series 2 over Bluetooth with the firmware whose descriptor xpadneo records (`045e:0b05`): the descriptor carries a whole boot keyboard after the game pad, and hid-microsoft puts every report on one input, so the pad's own node is a keyboard to udev, and not a joystick. The VM check makes one with that descriptor and sees it never hidden, a hidden pad's address notwithstanding.
- Any pad under the out-of-tree xpadneo driver, which adds a mouse and a keyboard input under the pad's HID device for its mouse mode: the device has a pointer among its inputs.
- A pad whose keys are all at 128 or above but whose words of 64 bits have only their low halves set, as udev reads them on a 64-bit system: with no word wider than 32 bits the rule can't tell the word size, and asks keys below 256 be absent as well. A pad with any joystick or game pad button (0x120 to 0x13f) has such a word; one with buttons only at `BTN_TRIGGER_HAPPY` and up has not. None is known.

**Risk.** Low. Such a pad still works in a setup, grabbed, and the tray and `ctrlassist setups` say it is grabbed. A program that opens a grabbed pad's event node, as a game or Steam through SDL does, hears nothing from it: what it sees is a second pad that says nothing, which a game may still count as a player. A raw node read around the grab only where one is open to the seat: there is none for a pad on xpad, which has no HID device; over Bluetooth hid-microsoft's raw node is root's under the stock rules, and Steam's rules (`60-steam-input.rules`) open only the Elite Series 2's over Bluetooth LE (`045E:0B22`) among Microsoft's pads.

**Mitigation.** On the pad, with its event node (`ls -l /dev/input/by-id/` lists it, as `…-event-joystick`):

```sh
udevadm info --query=property --name=/dev/input/eventN | grep -E '^ID_INPUT_(JOYSTICK|KEY|KEYBOARD|MOUSE|TOUCHPAD|TABLET)='
cat /sys/class/input/eventN/device/capabilities/key
```

`ctrlassist-hider check /dev/input/eventN` says `pad` for a pad the service hides, and `refused` otherwise.

**Closes when.** A real Xbox Series pad has been hidden and shown again over USB and over Bluetooth, and the pads still refused are either accepted as refused or told apart from a keyboard some other way.
