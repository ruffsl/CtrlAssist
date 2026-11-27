# GUI & Tray

CtrlAssist has two faces on a desktop. The **window** shows every controller and every profile, and is where a profile is made and changed. The **tray** is for the everyday: start, stop, hand over, and see what is running. Both only show what the daemon reports and pass on what you ask, so closing either changes nothing that runs.

## The CtrlAssist window

Open it from your application menu, or with **Settings** in the tray's menu, or a left click on the tray's icon. A second launch brings the open window to the front.

A sidebar on the left has the window's four pages. Under them, **Reload** has the daemon read its profiles again, and the tray its settings, for when a file was edited outside the window. The three dots at the bottom say whether the **Daemon**, the **Hiding** service and the **Tray** are running; [About](#about) says more of each.

The window follows your desktop's light or dark theme and its accent color. An **ⓘ** beside a section's title or at the end of a row explains it.

### Pads

![The Pads page, on a controller that a running profile is using](../docs/screenshots/pads_pane.png)

Every controller CtrlAssist knows is listed, the connected ones and those it remembers from before, and under **VIRTUAL** the virtual controllers that exist now. Pick one to see it.

- The chips at the top say how it is: **Connected** or **Disconnected**; **In couch**, with the name of the running profile that is using it; **Free**; **Remembered**; or **Virtual**.
- **Identify** buzzes the controller for a moment, so you can tell two apart. On a virtual controller it rumbles it as a game would, so the controllers behind it buzz the way its profile passes rumble on.
- **Visibility** says whether games can see this controller beside a virtual one: **Hidden**, **Grabbed** or **Visible**. The profile that is running with it decides; see [Profiles](./profiles.md#visibility).
- **PROFILES** lists every profile that names this controller, the part it plays there, and whether that profile is running. The pencil opens the profile.
- **INFO** has what tells this controller from others, with a button to copy each. A profile names a controller by its **Pad ID**, for this one and no other, or by its **Model**, for any of that kind.
- **Forget pad** drops a controller that isn't connected from the list.

![The same page on a virtual controller](../docs/screenshots/pads_pane_virtual.png)

A virtual controller is made by a profile while it runs, and games read it in place of the controllers behind it.

### Profiles

![The Profiles page, on a running profile](../docs/screenshots/profiles_pane.png)

Profiles are listed with the running ones first. The **+** at the top makes a new one, and the buttons beside a profile's name rename it, duplicate it, show its file and delete it. **default ✓** is always there and can't be renamed or deleted.

- **STATUS** has **Start** or **Stop**, with a line saying what games see. **Managed** has CtrlAssist start the profile itself whenever its controllers are connected.
- **INPUTS** is who plays which part. A controller that is unplugged, or busy in another running profile, can't be picked. The pencil shows that controller on the Pads page.
- **OUTPUTS** is the virtual controller, and what it presents itself as to games.
- **PRESET** is how the controllers are combined: the mode, who feels rumble, whose motion a game gets, and how the real controllers are kept from games.

What you pick is kept at once. A profile that is running goes on as it was, and its page says the profile has changed since it started: **Update…** then says what taking the change up costs, nothing a game notices or a restart that a game sees as its controller disconnecting and another connecting, and takes it up when you agree. [Profiles](./profiles.md) says what each choice means.

![A running profile in Toggle mode](../docs/screenshots/profiles_pane_running.png)

A profile in **Toggle** mode adds **Has control** while it runs: who plays now, and a pick to hand over.

A profile written by hand in the [full form](./configuration.md#the-full-form) shows none of these fields: the window lists it, starts and stops it, and shows its file.

### Settings

![The Settings page](../docs/screenshots/settings_pane.png)

**NOTIFY ME WHEN** has a switch for each kind of [notification](#notifications), and **Send a test notification** shows one now, whatever the switches say, to see that your desktop shows notifications at all. These are the tray's settings, so the tray has to be running to change them.

**COUCH MODE** is for using this window from across the room; see [below](#from-a-remote-or-a-gamepad).

### About

![The About page](../docs/screenshots/about_pane.png)

Each part of CtrlAssist, whether it is running, and the release it runs. A chip that isn't green says that part runs another release than this window, usually one left running from before an upgrade: restart it. **Config** opens the folder where a part keeps its files. **Diagnostics** copies a report of the releases, the system, the controllers and the profiles, for a bug report; controller addresses in it are shortened.

### From a remote or a gamepad

The window can be driven without a mouse: from a keyboard, or from a TV remote or a gamepad under a TV shell that turns their buttons into keys, as Plasma Bigscreen does. An accent-colored ring marks the control the keys act on. It appears with the first key press and hides again when you use the mouse.

| Key | What it does |
|---|---|
| **↑ ↓** | Move along the sidebar, down a list, or between the rows of a page. The sidebar switches pages as you move, and a list shows the controller or profile under the ring. |
| **← →** | Move along a row, from its control to the buttons beside it, and between the sidebar, the list and the page. |
| **OK** / **Enter** | Activate: press a button, flip a switch, open a choice, start typing in a field. |
| **Back** / **Esc** | One level out: close a help bubble, stop editing, leave the page for its list and the list for the sidebar. On the sidebar it returns from a jump you made with a pencil. |
| **Menu** | Help for the row under the ring, the same as its **ⓘ**. |
| **Tab** / **Shift+Tab** | The next or previous control; in couch mode, the next or previous page. |

Under Plasma Bigscreen, a gamepad's D-pad, Cross and Circle are the arrows, OK and Back, Square is Menu, and the shoulder buttons are Tab and Shift+Tab. CtrlAssist itself doesn't read a gamepad to steer its window: a running profile may have hidden the very controller in your hands.

**Couch mode.** On a TV across the room the window opens maximized and scaled up, and the keys are arranged for a remote: a text field or a number waits for OK before it takes the arrows, Tab and Shift+Tab step between the pages, and Back on the sidebar closes the window. It turns on by itself under Plasma Bigscreen. To use it on an ordinary desktop driving a TV, or to keep it off, set **Couch mode** on the Settings page; **Scale** makes everything larger or smaller on top of the display's own scale. It changes this window only: not the tray, and not what a profile does.

## The tray

The tray starts with your graphical session. Its icon shows trouble at a glance: a warning when an arrangement is waiting for a controller, an error when one has failed. Hovering over it lists what runs: each arrangement with its mode, its controllers, a ● beside the one that has control, and its virtual controller. After them come the profiles that are ready to start, and when nothing runs, how many controllers are connected.

A **middle click** on the icon starts or stops one profile without opening the menu: the one marked ► in that list. That is the profile you last started or stopped, and scrolling the **mouse wheel** over the icon moves the mark to another. The icon and the menu's icons come from your desktop's icon theme, in its flat single-color style where the theme has one, so they follow a light or dark panel.

### The menu

**Profiles** come first: each one you have kept, and anything running. **default ✓** is first: CtrlAssist always keeps it, so there is always a profile to fill in, and it can't be deleted or renamed.

- A profile that isn't running offers **Start**. One whose controllers aren't chosen yet says so instead.
- A running one shows its state in brackets and offers **Stop**, and for a toggle, **Has control**: who plays now, and a click to hand over. A controller that isn't connected can't be handed to.
- One that has failed says why. **Stop** clears it.
- Under those are the profile's settings, the same as the window's profile page: its controllers, what its virtual controller presents itself as, its mode, who feels rumble, how the real controllers are hidden, and whether it is managed. A profile written by hand in the full form has no settings here; while it runs its menu lists its controllers.

What you pick is kept at once. While the profile runs it also takes effect at once, without the game noticing: other controllers, a new mode, who feels rumble. A controller you swap out is given back to the system, and the new one is hidden. Picking another identity takes effect at once too, but needs a new virtual controller, which a game sees as its controller disconnecting and another connecting. A controller that is unplugged or busy in another arrangement can't be picked. Picking the one the other part has swaps the two, so the player and the helper change places in one click. The [Quick Start](../getting-started/quick-start.md) walks through it.

**Pads** lists every controller CtrlAssist knows, connected or remembered, and after them the virtual controllers that exist now.

- **Identify** buzzes a connected controller for a moment, so you can tell two apart. If a game is rumbling it, the buzz takes over briefly.
- **Identify** on a virtual controller rumbles it as a game would, so the controllers behind it buzz the way its profile passes rumble on: a quick check of who is mapped to it and who feels rumble.
- **Forget** drops a controller that isn't connected from the list.

The line after it, just above **Settings**, says how controllers are being hidden (`service` or `grab`, or `looking for the service…` for a moment after the daemon starts, while it hasn't yet heard from the hiding service) and whether the daemon got real-time scheduling. [How It Works](../reference/how-it-works.md) explains both.

**Settings** opens the [CtrlAssist window](#the-ctrlassist-window), or brings it to the front if it is open. A left click on the tray's icon does the same.

## Notifications

CtrlAssist tells you when:

- an arrangement **started** or **stopped** without your asking for it from the tray: a managed one starting by itself, or one started or stopped from the window or the command line. What you start or stop from the tray you can already see there;
- an arrangement **restarted** with a new virtual controller: because a change you picked needed one, or because it was restarted from the window or the command line;
- an arrangement is **held** because a controller disconnected, and when it is **running again**;
- an arrangement **failed**;
- the **daemon stopped**, which removes every virtual controller, or came back;
- an arrangement started with controllers that are **only grabbed, not hidden**, or with one that **can't rumble**;
- a controller that no arrangement uses **connected** or **disconnected**;
- something you asked for from the menu **couldn't be done**, with the reason.

Each kind can be turned off on the window's [Settings](#settings) page. The switches are kept in `tray.toml`, in `~/.config/ctrlassist` beside your profiles, and everything is on until you turn it off:

```toml
schema = 1
target = "default"
setup_started = true
setup_trouble = true
pad_connection = true
refused = true
```

- `setup_started`: an arrangement started or stopped.
- `setup_trouble`: held, failed, only grabbed, no rumble, the daemon stopped.
- `pad_connection`: a controller connected or disconnected.
- `refused`: something asked from the menu couldn't be done.
- `target`: the profile a middle click acts on, which the tray keeps there itself as the mark moves, so it is where it was after the tray restarts.

The tray reads the file when it starts, and again whenever the daemon comes back. After editing it by hand, **Reload** in the window takes the edit up, or says what is wrong with the file; until then the tray leaves a file it can't read as it is. The tray writes the file plain, so comments in it don't last.

## Starting and stopping the tray

The tray starts with your graphical session, as a service of yours. Stopping it leaves the daemon and anything running as they are; its notifications stop with it.

```sh
systemctl --user status ctrlassist-tray
systemctl --user restart ctrlassist-tray
```
