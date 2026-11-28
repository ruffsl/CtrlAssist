# A browser keeps the raw nodes of pads that are gone, and the machine has 64

The kernel has 64 raw HID nodes for the whole machine (`HIDRAW_MAX_DEVICES`), and a node's place is free again only once the last program that opened it has closed it, however long ago its device left. A device made when none is free gets no raw node, and no error says so. A virtual DualSense and a virtual Steam Controller are each read by games through their raw node, and a Steam Controller pad is read by the daemon through its own.

Chrome opens the raw nodes of pads it knows, and was seen to keep them open after the pads were gone.

**What was seen,** on 2026-11-28, Linux 7.3.4:

- **On the maintainer's desktop,** Chrome 157, open through a day of test runs, held 43 raw nodes whose pads were gone. With the machine's own 18 that left two places. A fake Steam Controller was then made without a raw node, nine of its tests failed, and one waited eight hours on a read. Closing Chrome freed all 43 at once.
- **In a trial,** Chrome 158, headless, with a profile of its own and one page that asks for the pads five times a second (`navigator.getGamepads()`), while the tests that make pads ran: it was left holding 12 nodes of pads that were gone after one run, 26 after two, 38 after three.
- **Which pads.** Sampled as the tests ran, Chrome opened fake DualSense pads on a cable and over Bluetooth, fake Steam Controllers on a cable and behind a Puck, and the daemon's own virtual Steam Controller, and kept most of each after they were gone: 11 of the 14 the sampling caught in one run. A virtual DualSense has the name and numbers of the fake one, so the two weren't told apart.
- **Tests fail before the places run out.** With that Chrome polling, between two and seven Steam Controller tests failed in each run. That is as [testing.md](../testing.md) says of any program that opens a fake Steam Controller's raw node: the driver takes the pad's input devices away while it is open.

**What wasn't.**

- **One pad at a time.** A fake DualSense, a fake Steam Controller, a virtual Steam Controller and a uinput pad, each made alone by the fixture and held for up to seven seconds, six times each: Chrome opened none of their raw nodes and kept none. What differs between that and a test run, in which Chrome opened dozens, isn't known: how many pads there are at once, how quickly they come and go, or something a test does to its pad.
- **One setup at a time.** A daemon with two uinput fakes behind a virtual DualSense, the setup started and stopped twenty times under the same Chrome and page: the virtual pad's raw node was there each time, and Chrome opened it not once and kept nothing. The same again with the machine's two real pads unplugged, and then with a virtual Steam Controller: nothing opened, nothing kept.
- **The tests one at a time.** With the real pads unplugged, the DualSense's tests and then the Steam Controller's, each run with one test at a time, left Chrome holding nothing. The same tests run together, as they are by default, left it holding eight.

**Why,** read in Chromium's source (`device/gamepad/gamepad_platform_data_fetcher_linux.cc`, as of 2026-11-28) and then seen:

- Chrome treats a raw node's removal as it treats its arrival: `OnDeviceRemoved` calls the same `RefreshDevice` as `OnDeviceAdded`, which for a raw node opens it by its path.
- Before opening, it asks whether the pad is one it reads this way, a DualShock 4, some Xbox pads and a few others, by the `HID_ID` of the node's parent. A DualSense and a Steam Controller aren't among them, which is why a pad alone was never opened. Where the parent can't be read, as of a node already removed, the question is skipped and the node is opened.
- So a removed pad's node is opened by its path, which is there only if another pad has been given the same number meanwhile. Chrome then holds the new pad's node under the old pad's name, which nothing later matches, and never closes it.
- **Seen:** fake DualSense pads made one after another with three seconds between, five times, and Chrome opened none. Each made as the last was removed, eight times, with never more than one at a time: Chrome opened the reused node and kept it after that pad was gone too, once in the eight.
- No report of it was found in Chromium's tracker. The list of pads it asks about came in a change of December 2025; before it every raw node was opened as it arrived.

**What isn't known.**

- **How often a quick swap is caught.** Once in eight by hand; the tests, with dozens of pads in seconds, lose about a dozen places a run.
- **A person's own use over days,** with a browser's own windows and tabs, not a headless one with one page.
- **Whether a page must ask for pads.** The trial's page did. Which of the maintainer's own tabs did, if any, wasn't looked at.
- **Other browsers** built on Chromium, which likely do the same, and other programs that read pads through raw nodes.

**Risk.** Low for a person playing. A place is lost only when a pad's raw node goes and another takes its number before Chrome has heard of the first, with a page open that asks for pads. A setup stopped and started again at once, fifteen times for each kind of virtual pad, lost none; started and stopped with a pause, twenty times, none. It isn't none: two setups changing together, or a pad that drops and returns as a setup starts, could be caught, each time one place until the browser is closed, and with none left the next virtual DualSense or Steam Controller has no raw node, which a game reading it that way sees as no pad, and a real pad plugged in has none either. High for whoever runs the tests on a desktop with a browser open on such a page: they fail, and for no reason a failure names.

**Mitigation.** Closing the browser frees every place it held, and [testing.md](../testing.md) names a browser among what to close before a run. A test that reads a virtual Steam Controller's raw node now fails within its own twenty seconds where it waited without end.

**What could be done,** none of it decided:

- A fake pad made without the raw node its test needs could say how many of the 64 places are taken, and by which programs where that can be read.
- The daemon could say the same where a virtual pad it makes has no raw node, if it doesn't already notice: unchecked.
- It is Chromium's to fix: a removal shouldn't open anything, and a pad whose parent can't be read shouldn't pass for one on the list. It hasn't been reported; a report is drafted below.

**Closes when** Chromium no longer opens a raw node on its removal, in a release the distributions carry; or, short of that, when a fake pad made without the raw node its test needs says why.

## A report for Chromium, drafted and not filed

For the maintainer to file at [issues.chromium.org](https://issues.chromium.org) (component Blink>GamepadAPI), in their own words where these don't fit. That the parent's `HID_ID` can't be read for a removed node is inferred from the source and from what was seen, not observed directly; the report says so.

> **Title:** Linux: a removed hidraw gamepad node is reopened by path and never closed, leaking a hidraw minor
>
> **Chrome version:** 158 (also seen in 157). **OS:** Linux 7.3, x86_64.
>
> **Summary.** On Linux, when a hidraw device is removed and another HID device is given the same hidraw minor before Chrome handles the removal, Chrome opens the new device's `/dev/hidrawN` and keeps the descriptor open after that device is removed too, until the browser exits. The kernel has 64 hidraw minors (`HIDRAW_MAX_DEVICES`) and frees one only when its last descriptor closes, so each leak takes one from the whole machine. With all 64 taken, a HID device plugged in gets no hidraw node at all, and nothing reports an error.
>
> **Steps to reproduce.**
>
> 1. Open a page that polls the Gamepad API, for example one that calls `navigator.getGamepads()` every 200 ms.
> 2. Create and destroy HID gamepads so that each is created as the previous one is removed, and so takes its hidraw minor. We did it with uhid devices carrying a DualSense's descriptor and IDs (`054c:0ce6`), one alive at a time, eight in a row.
> 3. Destroy the last one, then look at what Chrome holds: `ls -l /proc/<pid>/fd | grep hidraw` for the browser's processes.
>
> **Expected.** Chrome holds no hidraw descriptor: the device isn't on the hidraw allowlist (`kAllowlistHidrawGamepads`), and every device is gone.
>
> **Actual.** Chrome holds `/dev/hidrawN (deleted)`, and keeps it until it exits. In our trial one of eight swaps leaked; with three seconds between devices, none of five did. A test suite that creates dozens of such devices in parallel left Chrome 158 (headless, fresh profile, that one page) holding 12, 26 and 38 deleted nodes after one, two and three runs, and a Chrome 157 in daily use held 43.
>
> **Cause, as we read it** in `device/gamepad/gamepad_platform_data_fetcher_linux.cc`:
>
> - `OnDeviceRemoved` calls `RefreshDevice`, the same as `OnDeviceAdded`, and for a hidraw node `RefreshDevice` calls `RefreshHidrawDevice`, which opens the node by its path (`OpenHidrawNode`).
> - `RefreshHidrawDevice` applies the allowlist only if `GetVendorProduct` returns a value. `GetVendorProduct` reads `HID_ID` from the parent udev device; for a removed device we believe that fails (inferred, not observed), and the node is then opened unfiltered.
> - For a removed node the open normally fails, as the path is gone. If another device has taken the same minor it succeeds, on the new device's node. That descriptor is stored under the removed device's syspath, which no later event matches, so it is never closed.
>
> **Suggested fix.** Don't open anything on a remove event, and treat a device whose vendor and product can't be read as not on the allowlist.
