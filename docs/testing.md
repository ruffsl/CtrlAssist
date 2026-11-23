# Testing

How the workspace is tested and where a new test goes. The approach is TisVlay's: tests are hermetic, the operating system's side is faked at the seam nearest the kernel, and what can't be reached that way is checked on hardware and said so.

## Layers

| Layer | Tests | Runs in |
|---|---|---|
| Routing and feedback | Scenarios and property tests in `ctrlassist-core`: pad states in, changes and rumble levels out, no devices | `just test`, everywhere |
| Device code | Against fake pads from `ctrlassist-test` | `just test` where uinput and uhid are writable; the virtual machine check otherwise |
| The daemon | Against a live daemon on a temporary socket, with fake pads and a stand-in for the hiding service; then the built command against the real service | `just test`; `just vm daemon` |
| The platform | What a clean machine gives a seat user | `just vm platform` |
| The settings window | Its element tree against synthetic models, driven by keys, through Slint's testing backend; then the window as packaged | `just test`; `just vm gui` |
| Hardware | What none of the above can reach | By hand, listed in the change that needs it |

## Rules

- **nextest is the runner,** with one integration-test binary per crate (`tests/it/main.rs` and its modules), which links once instead of once per file. `ctrlassist-gui` has a single test file instead, `tests/theme_breeze.rs`, which is that binary by itself.
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
| `synthetic_dualsense` | nothing | A report descriptor of the test crate's own that declares the reports the kernel's PlayStation driver exchanges, by number and size, and a firmware report for either rumble layout | A connected DualSense's copied data, none of which is in the repository: the driver binds a virtual pad made from it as it does one made from the real thing |
| `FakeHidPad` | uhid | Those, plus a HID device with a driver bound, a hidraw node and an address | A PlayStation pad, any Bluetooth pad, anything whose driver is re-bound or whose hidraw node is read |

Every fake pad is marked as the tests': one made through uinput or uhid by where it says it is plugged (`ctrlassist/test/…`), and `FakeDualSense`, whose driver carries no such string to its input devices, by an address that starts `02:ca:7e:`. A daemon takes a marked pad for a pad to play with only when asked to (`--test-pads`, or `CTRLASSIST_TEST_PADS` set), which the tests' own daemons, the bench and the virtual machine checks do. So the suite can run beside a daemon in use, the dev stack's or an installed one, without filling its list of pads or starting its profiles. Other programs still see them: a running Steam adopts each one.

A clean machine opens neither interface to an ordinary user, so a test that needs one checks `can_make_pads()`, `uinput_available()` or `uhid_available()` and skips where it is missing, writing a `SKIPPED` line that says why. With `CTRLASSIST_REQUIRE_FAKE_PADS` set it fails instead; CI sets it, after opening uinput and the event nodes on its runner. The virtual machine checks run as root and never skip.

`ctrlassist-fixture` is the same fakes as a command, for the virtual machine checks' scripts: it creates a pad and holds it until killed, lists pads with what the caller can open, counts what arrives on a node, reads a pad as the daemon does, forwards a pad to a virtual pad, and hides a pad through the hiding service.

## Wire tests

The control plane is tested on real links: a daemon, or a stand-in for one, listens on a temporary socket under `/tmp` (a path under the build tree can exceed the kernel's limit for a socket's name) and a client calls it. The CLI's tests run the built `ctrlassist` binary against such a daemon, and start and stop the daemon as its service verb.

## Virtual machine checks

Each `packaging/nix/test-*.nix` is a NixOS test exposed as a flake check and run with `just vm <name>`. They need KVM. A check asserts; it doesn't just print. They start from a machine with only systemd's own udev rules, because a developer's desktop has Steam's and others' rules installed and hides what a clean machine refuses.

| Check | Asserts |
|---|---|
| `daemon` | CtrlAssist as its NixOS module installs it: the command, the units, the udev rule and the completions are there; the daemon is the seat user's service, started with the session, with its input thread real-time; the tray stays up with no tray host; the daemon finds the hiding service and the pads; a wrong profile is refused with where; a started setup's pad with a HID driver is hidden by the installed service, which runs as its own user with no capabilities and writes no rule, and its pad with no driver to re-bind is refused by the service and grabbed, while its virtual pad is the user's to read and their input reaches it; a hidden pad that reconnects is never visible and goes back into its setup; a changed profile is taken as the setup runs, on the virtual pad it has, and its pads change parts staying hidden; stopped, the pads are back as they were, with no marker left, and the re-bind that showed the pad again doesn't read as the pad leaving; a connected DualSense, stood in for by a virtual one with an address a real pad could have, is copied through the service and kept, and a setup's virtual pads are made from it as the kernel driver's own pads, the user's to read; a killed daemon's pads are restored and its virtual pad removed; its unit restarts it and it starts a profile that starts by itself, which stays stopped when stopped by hand; stopping the daemon restores the pads |
| `gui` | The settings window as the module installs it: its binary, its desktop entry and the session bus's service file, with the package's own path in it, are in place; the session bus starts the window when asked, as the tray and a launcher ask, and it takes its name on the bus; a second launch hands over to the window that is open and ends, leaving one; and the window is drawn, on a compositor rendering in software |
| `device` | With the packaged udev rule: a seat user reads a pad's report frames from a descriptor as pad states; the rule gives uinput and uhid to the seat user and nobody else; a virtual pad of each identity carries what the pad behind it does; the input thread merges two pads into one and is never scheduled while they are silent; a pad unplugged and plugged in again is read again; an unplugged pad is reported as gone |
| `hider` | The hiding service as packaged, with its unit's hardening and the shipped rule. Among what its subtests assert, each named for what it shows: it runs as its own user with every capability set empty, won't run as root, and writes no rule; it tells a pad from a keyboard and from what isn't a device; it serves only the user in front of the machine, and only for a pad on that user's seat, and a descriptor opened `O_PATH` names no pad; hiding re-binds the pad's driver (a HID pad's, and the USB interface of an Xbox 360 pad under xpad), closes its nodes to the seat user and cuts off whoever had it open, while another pad of the same model is untouched and the descriptor passed delivers input; a pad that reconnects is hidden before anyone can see it; restore, a killed client and a killed service each give the pad back and leave no marker; a raw node is opened only for a pad of the PlayStation driver, and a hidden one's stays closed to the seat user though a rule like Steam's opens it; a hostile pad name reaches no marker's name, and a forged twin doesn't get the real pad; a pad with no driver to re-bind is refused, for the daemon to grab; clients that never read and many idle connections don't stop it serving others; and run by hand with `--listen`, as `just hider-dev` runs it, it is the same service |
| `platform` | A seat user can open a pad's event and js nodes, and can't open `/dev/uinput`, `/dev/uhid` or a hidraw node; both kinds of fake pad deliver what they are sent |

## Hardware checks

What no virtual machine has is checked by hand on real pads, through the real service: `just hider-dev` runs the hiding service from the build tree as its own user, which needs the tree's udev rule and that user installed (on NixOS, the module from the tree), and `just hide-pad <node>` hides a pad through it and holds it hidden until Ctrl-C. `ctrlassist-hider check <node>` prints what the service makes of a node without changing anything. `just bench` times the input path. `just forward-pad <node>` puts a virtual pad in front of a real one, to see what Steam and SDL make of each identity.

## GUI testing

`ctrlassist-gui-app/tests/it` makes an `AppWindow` against synthetic models and queries its element tree through `i-slint-backend-testing`'s `ElementHandle`. The tests call `init_no_event_loop()`, so many `#[test]` functions share a binary with no daemon, no wire and no event loop: they answer "does the window show the right thing for this state, and do the keys go where they should?". One test, `a_snapshot_reads_a_live_daemon_and_its_going`, starts a real daemon and input thread instead, and reads the window's state from it.

`ctrlassist-gui/tests/theme_breeze.rs` reads `ui/theme.slint` as text and checks each fixed color token against the Breeze role it is taken from ([gui/theme-breeze.md](gui/theme-breeze.md)).

### Required build setting

Slint's compiler writes element ids into the generated code only when told to at build time, and without them `find_by_element_id` and `accessible_id()` silently find nothing, so a test looks as if the window were empty. `crates/ctrlassist-gui/build.rs` turns them on for every profile but the release-derived ones, so `just build`, a bare `cargo test`, rust-analyzer and CI all have them with no environment variable, and the packages ship without. The build script's setting overrides `SLINT_EMIT_DEBUG_INFO`.

### Naming

- `accessible-label`, translated with `@tr(…)`, is what a screen reader announces; on buttons it defaults to the visible text. Tests find it with `ElementHandle::find_by_accessible_label`.
- `accessible-id`, a literal `<pane>-<element>` (`pads-row-…`, `settings-couch-mode`, `sidebar-about`), is the stable selector. The testing backend has no finder for it, so `tests/it/support.rs` has `find_by_accessible_id`, which takes the first match, and `find_all_by_accessible_id`: Slint passes accessibility properties down to descendants, so one widget answers several times, and a pane that makes a detail per row has a control of that id in each. `activate_shown` presses the one of them that is showing and enabled.

### Driving keys

The testing backend has no event loop and no key helper, but `slint::Window::dispatch_event` needs neither: a pressed and released pair runs Slint's capture and bubble handlers and its own Tab traversal at once. `tests/it/support.rs` wraps it as `press(&app, Key::Tab)`, `press_text` for typed characters, `press_shift_tab`, which holds Shift around a Tab as a keyboard or a gamepad's shoulder button does (`Key::Backtab` never arrives from a real window), and `press_with_alt`. Each ends with `settle()`, which advances the mock clock a frame and fires the timers due, so deferred focus and synthesized keys have run before the assertion; `elapse` advances further, for an animation such as a wheel scroll. A control is observed through what it does: Return on a switch fires its callback and flips `accessible_checked()`, typed text lands in a field's `accessible_value()`. Prefer effects on state the test set up itself. The key navigation itself is described in [gui/remote-navigation.md](gui/remote-navigation.md).

### Not done on purpose

- **No pixel snapshots.** Slint ships none in its testing backend, and pixel comparisons are the most flake-prone kind of test; the alternative (the software renderer, bundled fonts and a tolerance) is heavy upkeep for little signal.
- **No tests through Slint's MCP server.** It is for looking at the window while developing (`.claude/slint.md` has its quirks); tests use the in-process testing backend only.

### Pin

`i-slint-backend-testing` is pinned exactly to Slint's release, since its API carries no semver promise and moves helpers between modules across minor releases. A Slint bump re-pins it deliberately.

## zenoh in tests

- **A multi-threaded runtime.** zenoh refuses Tokio's current-thread scheduler, so every test that opens a session is `#[tokio::test(flavor = "multi_thread")]`.
- **No ordering across keys.** Samples on different keys may arrive in any order, so a test waits on one subscription for everything it needs rather than on several in turn.
- **History only through the advanced subscriber.** What the daemon publishes reaches a late joiner only through an `AdvancedSubscriber` with history, as the SDK's mirror uses; a plain subscriber sees only live samples, and a `get` doesn't read the cache.
- **Deadlines, not sleeps.** A wait for a sample or a state is bounded and fails with what it saw, so a hang is a failure that says why, not a stalled run.
