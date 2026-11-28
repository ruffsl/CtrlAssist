# Quick Start

This pairs two controllers so that a helper can assist a player, in about a minute. It uses the CtrlAssist GUI; the [tray](#from-the-tray) does the same in a menu.

## From the GUI

1. Connect both controllers, and open **CtrlAssist** from your application menu.
2. On the **Pads** page, check that both are listed. Not sure which is which? Pick one and press **Identify**: it buzzes.

   ![The Pads page, listing the connected controllers](../docs/screenshots/pads_pane_light.png)

3. Go to **Profiles** and pick **default ✓**, the first entry. It is a profile CtrlAssist always keeps, ready for you to fill in.
4. Under **INPUTS**, set **Primary** to the main player's controller and **Assist** to the helper's.
5. Under **PRESET**, pick the **Mode**, which is how the two combine:
   - **Priority**: the primary plays, and the assist takes over whatever it touches.
   - **Average**: sticks and triggers are blended between the two.
   - **Toggle**: one plays at a time; the Mode (guide) button, or **Has control** in the GUI, hands over.
6. Press **Start**, under **STATUS**.

   ![A profile running, with its Stop button](../docs/screenshots/profiles_pane.png)

A new controller appears to games, and the two real ones disappear from them. Start your game and choose the new controller if it asks.

To stop, press **Stop**. The real controllers come back.

Two more choices on the same page:

- **Rumble** picks who feels the game's rumble: both, the primary only, the assist only, or neither.
- **Managed** has CtrlAssist start this arrangement itself whenever both controllers are plugged in, from then on. Unmanaged, you start it.

Your choices are the profile: they are kept as you make them. [Profiles](../guide/profiles.md) says what every choice means, and [GUI & Tray](../guide/gui-tray.md) walks through the rest of the GUI.

## From the tray

The same profile is in the tray's menu, for when you'd sooner not open a window.

1. Click the CtrlAssist tray icon and open **default ✓**.
2. Pick the controllers under **Primary** and **Assist**, and how they combine under **Mode**.
3. Click **Start**, at the top of the same menu. It appears once both controllers are picked.

To stop, open **default ✓** again and click **Stop**. From then on a middle click on the tray icon starts and stops it, without opening the menu.

<details><summary id="from-a-terminal">From a terminal</summary>

The same thing as a file, saying who plays which part:

```toml
# couch.toml
mode = "priority"
primary = "model:054c:0ce6"   # any DualSense
assist = "model:045e:0b12"    # any Xbox Series pad
```

`ctrlassist pads` lists your controllers and what to call them. Then:

```sh
ctrlassist profile put couch.toml
ctrlassist start couch
ctrlassist setups          # what is running
ctrlassist stop couch
```

[Configuration Files](../guide/configuration.md) has everything a profile can say, and [Command Line](../guide/cli.md) every command.

</details>

## If a controller disconnects

The game keeps its controller. CtrlAssist holds the arrangement: the missing pad reads as untouched, the other plays on, and a notification says so. Reconnect the pad and it takes its place again.
