# A pad udev also marks as a keyboard is grabbed, not hidden

The hiding rule gives the service's group a pad's `uevent` file, through which the service has its driver re-bound, only for an event node that udev marks as a joystick and as nothing a keystroke or a pointer could come from: one with `ID_INPUT_KEYBOARD`, `ID_INPUT_KEY`, `ID_INPUT_MOUSE`, `ID_INPUT_TOUCHPAD` or `ID_INPUT_TABLET` set is passed over (`packaging/udev/72-ctrlassist-hide.rules`). The service makes the same test before it hides anything (`is_only_a_pad` in `crates/ctrlassist-hider/src/pad.rs`) and refuses such a node as not a pad, so the daemon grabs it instead. Both are meant for a keyboard that shares a HID device with a pad, which a re-bind would cut off too.

udev sets `ID_INPUT_KEY` on an input device with any key code from a keyboard's ranges, not only on keyboards. A pad whose driver reports one key of that kind on the pad's own event node is then never hidden. The Xbox Series pad may be one: its Share button is reported as a key on the keyboard's side of the range (`KEY_RECORD`, by the xpad driver on USB, as remembered and not read again here). That is the pad the guide's first steps use. Not observed on any pad: the Xbox One pad on USB that hiding was tried on has no Share button.

**Risk.** Low to medium. Such a pad still works in a setup, grabbed, and the tray and `ctrlassist setups` say it is grabbed; but a program that reads it some other way, as Steam does, still sees it, which is what hiding exists to stop.

**Mitigation.** On the pad, with its event node (`ls -l /dev/input/by-id/` lists it, as `…-event-joystick`):

```sh
udevadm info --query=property --name=/dev/input/eventN | grep -E '^ID_INPUT_(JOYSTICK|KEY|KEYBOARD|MOUSE|TOUCHPAD|TABLET)='
```

The pad is affected if that prints `ID_INPUT_JOYSTICK=1` together with `ID_INPUT_KEY=1` or `ID_INPUT_KEYBOARD=1` (or any of the others) for the same node. Only `ID_INPUT_JOYSTICK=1` means it is hidden as any pad is. `ctrlassist-hider check /dev/input/eventN` agrees: `refused` for an affected pad, `pad` otherwise.

**Closes when.** The command has been run on an Xbox Series pad over USB and over Bluetooth. If either is affected, the rule and the service tell a key on the pad's own event node from another input device under the same HID device that is a keyboard or a pointer, and a test covers a pad with such a key.
