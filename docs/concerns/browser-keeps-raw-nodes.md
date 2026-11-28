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
- **A person's own use:** one setup started and stopped, with a browser open, over days. Whether its virtual pad's raw node is one Chrome opens and keeps is the question that matters, and the trial doesn't answer it.
- **Whether a page must ask for pads.** The trial's page did. Which of the maintainer's own tabs did, if any, wasn't looked at.
- **Other browsers,** and other programs that read pads through raw nodes.

**Risk.** Unknown for a person playing, until one setup at a time is tried. If Chrome keeps a virtual pad's raw node as it kept the tests', each start of a setup costs a place until the browser is closed, and after some forty, the next virtual DualSense or Steam Controller has no raw node, which a game reading it that way sees as no pad, and a real pad plugged in has none either. High for whoever runs the tests on a desktop with a browser open: they fail, and for no reason a failure names.

**Mitigation.** Closing the browser frees every place it held. A test that reads a virtual Steam Controller's raw node now fails within its own twenty seconds where it waited without end.

**What could be done,** none of it decided:

- A fake pad made without the raw node its test needs could say how many of the 64 places are taken, and by which programs where that can be read.
- The daemon could say the same where a virtual pad it makes has no raw node, if it doesn't already notice: unchecked.
- [testing.md](../testing.md) could name a browser beside Steam among what to close before a run.
- If Chrome keeps the node of a pad that is simply unplugged, that is Chromium's to fix, and theirs to be told.

**Closes when** a setup with a virtual DualSense, and one with a virtual Steam Controller, has been started and stopped some twenty times with Chrome open on a page that asks for pads, and the nodes Chrome holds counted after each (`ls -l /proc/<pid>/fd | grep hidraw`, the browser's own process): either it keeps none, and this is the tests' concern alone, or it keeps them, and the daemon says so where a virtual pad gets no raw node and the user guide's troubleshooting page says what to close.
