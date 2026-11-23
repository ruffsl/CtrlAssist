# Spike: motion and the touchpad on a virtual DualSense

Throwaway code for the first step of [the motion and touch plan](../../docs/plans/motion-and-touch.md). It answers: what does it take for one real DualSense's gyroscope, accelerometer and touchpad to reach a game through a virtual DualSense, and can the game tell?

`src/main.rs` builds `motion`. It hides one DualSense through the installed hiding service, takes its event node and its raw HID node, makes a virtual DualSense through uhid, and for every input report the pad sends writes one to the virtual pad: buttons, sticks and triggers from the pad's event node, as the daemon reads them; the two sensors, their timestamp, both touch points and the touchpad's click from the pad's raw report. Output reports games send the virtual pad (rumble, lights, trigger effects) are passed on to the pad whole, through the device crate. Ctrl-C ends it: the virtual pad goes, the pad is left at rest and shown again.

It reuses the device crate's pad reader, its shipped DualSense descriptor and firmware report, its writer of output reports, and the hiding service's client. The uhid emulator is the device crate's, copied and trimmed (`src/virtual_pad.rs`): that one writes only what a pad state holds, and always gives the neutral calibration.

## Running it

From this directory, with the pad connected and not hidden (no setup using it), and the installed daemon stopped so that none of its setups takes the pad meanwhile:

```fish
systemctl --user stop ctrlassist-daemon.service
nix develop ../.. -c cargo run --offline -- --record /tmp/motion.csv
```

It logs to standard error; `RUST_LOG=debug` adds each feature report asked of the virtual pad. Every thousand reports it logs the mean time between them, by the pad's clock and as they arrived, and the longest gap.

| Flag | Values | What it varies |
|---|---|---|
| `--motion` | `pad`, `none` | The pad's sensors, or a pad lying flat and still, as the daemon's virtual pad reports now. |
| `--calibration` | `sdl`, `kernel`, `copy`, `raw` | How the pad's sensor counts become the virtual pad's; below. |
| `--clock` | `pad`, `arrival` | The virtual pad's sensor clock: its own, advanced by the time between the pad's readings; or the time each report arrived, as the daemon's virtual pad stamps them now. |
| `--touch` | `pad`, `none` | The pad's two contacts and click, or none. |
| `--hide` | `service`, `none` | `none` grabs the event node and opens the raw node by its path, for a machine without the service; games then see the pad as well. |
| `--pad` | an event node | The first DualSense connected if not given. |
| `--record` | a file | Every report as CSV: when it arrived (µs), the pad's and the virtual pad's ticks (thirds of a µs), the pad's six sensor counts and the six written, and the two contacts and the click. A written count is worth 1080/17678 °/s for the gyroscope and 1/8192 g for the accelerometer under the neutral calibration. |

## Calibration

The pad's raw report carries uncalibrated counts. Whoever reads a DualSense applies the calibration feature report (0x05) it reads off that pad: the kernel's driver for its motion node, SDL and Steam for a game that reads the raw node. A virtual pad's readers read the virtual pad's report, which the emulator answers. So what the virtual pad says decides how often a count is calibrated:

- `copy`: the virtual pad gives the real pad's own report and the counts pass unchanged. Every reader calibrates once, exactly as it would the real pad. It works for one pad only: the report is read once, as the driver binds and as a game opens the pad.
- `sdl` and `kernel`: the virtual pad gives the device crate's neutral report; the spike turns the pad's counts into turning and gravity by the pad's report, as SDL reads it (gyroscope bias subtracted) or as the kernel's driver does (not), and back into counts by the neutral report. Calibrated once, by the spike, whatever pad the counts came from.
- `raw`: the neutral report and the counts unchanged. The pad's own calibration is never applied; the neutral one is a usual pad's, so the error is the pad's departure from usual.

The neutral report is not one count to one unit for the gyroscope: both readers take it as 1080 °/s over 17678 counts. A count that a pad's driver has already calibrated, from its motion node in units of 1/1024 °/s, written as is, would turn about 62 times too fast; the accelerometer's 8192 to the g happens to be the identity.

Read from the kernel's `drivers/hid/hid-playstation.c` and SDL's `src/joystick/hidapi/SDL_hidapi_ps5.c` upstream on 2026-11-23; Steam's own reader and older SDL2 builds in games were not read.

## What it leaves out

One pad, a DualSense, and nothing mixed; no graph. A DualShock 4's report is laid out otherwise and isn't read. The virtual pad reports when the pad does, so it needs no beat of its own while the pad is there.
