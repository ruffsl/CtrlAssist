# Quick Start

This pairs two controllers so that a helper can assist a player, in about a minute.

## From the tray

1. Connect both controllers.
2. Click the CtrlAssist tray icon and open **default ✓**, the first entry. It is a profile CtrlAssist always keeps, ready for you to fill in.
3. Under **Primary**, pick the main player's controller. Under **Assist**, pick the helper's.
   - Not sure which is which? Open **Pads**, pick one, and choose **Identify**: it buzzes.
4. Under **Mode**, pick how the two combine:
   - **Priority**: the primary plays, and the assist takes over whatever it touches.
   - **Average**: sticks and triggers are blended between the two.
   - **Toggle**: one plays at a time; the Mode (guide) button, or **Has control** in the tray, hands over.
5. Click **Start**, at the top of the same menu. It appears once both controllers are picked.

A new controller appears to games, and the two real ones disappear from them. Start your game and choose the new controller if it asks.

To stop, open **default ✓** again and click **Stop**. The real controllers come back. From then on a middle click on the tray icon starts and stops it, without opening the menu.

Two more choices in the same menu:

- **Rumble** picks who feels the game's rumble: both, the primary only, the assist only, or neither.
- **Managed** has CtrlAssist start this arrangement itself whenever both controllers are plugged in, from then on. Unmanaged, you start it.

Your choices are the profile: they are kept as you make them, and the settings window shows the same profile under **Profiles**.

## From the command line

The same thing as a profile, a short file saying who plays which part:

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

[Configuration Files](../guide/configuration.md) has everything a profile can say.

## If a controller disconnects

The game keeps its controller. CtrlAssist holds the arrangement: the missing pad reads as untouched, the other plays on, and a notification says so. Reconnect the pad and it takes its place again.
