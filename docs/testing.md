# Testing

How the workspace is tested and where a new test goes. The approach is TisVlay's: tests are hermetic, the operating system's side is faked at the seam nearest the kernel, and what can't be reached that way is checked on hardware and said so.

## Layers

| Layer | Tests | Runs in |
|---|---|---|
| Routing and feedback | Scenarios and property tests in `ctrlassist-core`: pad states in, changes and rumble levels out, no devices | `just test`, everywhere |
| Device code | Against fake pads from `ctrlassist-test` | `just test` where uinput and uhid are writable; the virtual machine check otherwise |
| The daemon | Against a live daemon on a temporary socket, with fake pads and a stand-in for the hiding service; then the built command against the real service | `just test`; `just vm daemon` |
| The platform | What a clean machine gives a seat user | `just vm platform` |
| Hardware | What none of the above can reach | By hand, listed in the change that needs it |

## Rules

- **nextest is the runner,** with one integration-test binary per crate (`tests/it/main.rs` and its modules), which links once instead of once per file.
- **No retries.** A flaky test is quarantined with `#[ignore = "flaky: …"]` and a note of why, never retried into passing.
- **A bug fix starts with a test that reproduces it,** where one can.
- **Property tests check against a model written the obvious way,** not the implementation's arithmetic written twice, and assert that they are exercising real cases: a property that only ever sees refusals passes for the wrong reason.
- **A parser of outside input gets a never-panics property test.** A panic on the input thread takes every virtual pad with it.
- **Coverage has a floor** that only rises; see [build.md](build.md#ci).

## Fake pads

`ctrlassist-test` makes pads through the kernel's own injection interfaces, so the code under test runs against real device nodes rather than a mock of the input library:

| Fake | Made through | Has | Stands in for |
|---|---|---|---|
| `FakePad` | uinput | An event node and a js node; buttons, two sticks, two triggers, a hat | Any evdev gamepad; a pad with no HID driver |
| `RumblePad` | uinput | An event node; two buttons, a stick, and rumble motors that note what they were told | A pad a buzz or a game's rumble must reach |
| `FakeHider` | a socket | The hiding service's protocol, hiding nothing: it hands back the pad's node and notes what it was asked | The hiding service, for what the daemon asks of it and when |
| `FakeHidPad` | uhid | Those, plus a HID device with a driver bound, a hidraw node and an address | A PlayStation pad, any Bluetooth pad, anything whose driver is re-bound or whose hidraw node is read |

A clean machine opens neither interface to an ordinary user, so a test that needs one checks `can_make_pads()`, `uinput_available()` or `uhid_available()` and skips where it is missing, writing a `SKIPPED` line that says why. With `CTRLASSIST_REQUIRE_FAKE_PADS` set it fails instead; CI sets it, after opening uinput and the event nodes on its runner. The virtual machine checks run as root and never skip.

`ctrlassist-fixture` is the same fakes as a command, for the virtual machine checks' scripts: it creates a pad and holds it until killed, lists pads with what the caller can open, counts what arrives on a node, reads a pad as the daemon does, forwards a pad to a virtual pad, and hides a pad through the hiding service.

## Wire tests

The control plane is tested on real links: a daemon, or a stand-in for one, listens on a temporary socket under `/tmp` (a path under the build tree can exceed the kernel's limit for a socket's name) and a client calls it. The CLI's tests run the built `ctrlassist` binary against such a daemon, and start and stop the daemon as its service verb.

## Virtual machine checks

Each `packaging/nix/test-*.nix` is a NixOS test exposed as a flake check and run with `just vm <name>`. They need KVM. A check asserts; it doesn't just print. They start from a machine with only systemd's own udev rules, because a developer's desktop has Steam's and others' rules installed and hides what a clean machine refuses.

| Check | Asserts |
|---|---|
| `daemon` | CtrlAssist as its NixOS module installs it: the command, the units, the udev rule and the completions are there; the daemon is the seat user's service, started with the session, with its input thread real-time; the tray stays up with no tray host; the daemon finds the hiding service and the pads; a wrong profile is refused with where; a started setup's pads, one with a HID driver and one without, are hidden by the service while its virtual pad is the user's to read, and their input reaches it; a hidden pad that reconnects is never visible and returns to its setup; stopping restores the pads and leaves no rule; a killed daemon's pads are restored and its virtual pad removed; its unit restarts it and it starts the profile by itself; stopping the service restores the pads |
| `device` | With the packaged udev rule: a seat user reads a pad's report frames from a descriptor as pad states; the rule gives uinput and uhid to the seat user and nobody else; a virtual pad of each identity carries what the pad behind it does; the input thread merges two pads into one and is never scheduled while they are silent; a pad unplugged and plugged in again is read again; an unplugged pad is reported as gone |
| `hider` | The hiding service as packaged, hardening included: a hidden pad is denied and its earlier reader cut off; the other pad of the same model is untouched; the passed descriptor delivers input; a reconnecting pad is never visible; restore, a killed client and a killed service each give the pad back and leave no rule; a user not in front of the machine, a keyboard and a hostile pad name get nowhere; a pad with no driver is restored exactly |
| `platform` | A seat user can open a pad's event and js nodes, and can't open `/dev/uinput`, `/dev/uhid` or a hidraw node; both kinds of fake pad deliver what they are sent |

## Hardware checks

What no virtual machine has is checked by hand on real pads, through the real service: `just hider-dev` runs the hiding service from the build tree as root, and `just hide-pad <node>` hides a pad through it and holds it hidden until Ctrl-C. `ctrlassist-hider check <node>` prints what the service makes of a node without changing anything. `just bench` times the input path. `just forward-pad <node>` puts a virtual pad in front of a real one, to see what Steam and SDL make of each identity.
