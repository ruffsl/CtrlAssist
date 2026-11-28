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

**What isn't known.**

- **What it is about many pads at once.** Chrome opens and keeps raw nodes when the tests run together and not when they run singly, so it is how many pads there are at a time, or how fast they come and go, and neither was measured: a try at making several fake DualSense pads at once from the fixture made none that counted, all having one address.
- **A person's own use over days,** with a browser's own windows and tabs, not a headless one with one page.
- **Whether a page must ask for pads.** The trial's page did. Which of the maintainer's own tabs did, if any, wasn't looked at.
- **Other browsers,** and other programs that read pads through raw nodes.

**Risk.** Low for a person playing: a setup's virtual pad, of either kind, cost no place in twenty starts, with real pads connected and without. Not none, while how many pads at once it takes is unknown: someone with several setups running, their pads coming and going together, is nearer the tests than the trial was. Where Chrome does keep a virtual pad's raw node, each costs a place until the browser is closed, and with none left the next virtual DualSense or Steam Controller has no raw node, which a game reading it that way sees as no pad, and a real pad plugged in has none either. High for whoever runs the tests on a desktop with a browser open: they fail, and for no reason a failure names.

**Mitigation.** Closing the browser frees every place it held. A test that reads a virtual Steam Controller's raw node now fails within its own twenty seconds where it waited without end.

**What could be done,** none of it decided:

- A fake pad made without the raw node its test needs could say how many of the 64 places are taken, and by which programs where that can be read.
- The daemon could say the same where a virtual pad it makes has no raw node, if it doesn't already notice: unchecked.
- [testing.md](../testing.md) could name a browser beside Steam among what to close before a run.
- If Chrome keeps the node of a pad that is simply unplugged, that is Chromium's to fix, and theirs to be told.

**Closes when** how many pads at once, or how fast, it takes for Chrome to open and keep their raw nodes is known (`ls -l /proc/<pid>/fd | grep hidraw`, the browser's own process, as pads are made and removed in step): either it is more than setups make, and this is the tests' concern alone, or setups can reach it, and the daemon says so where a virtual pad gets no raw node and the user guide's troubleshooting page says what to close.
