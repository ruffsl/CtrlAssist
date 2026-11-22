# CtrlAssist

CtrlAssist lets several game controllers act as one, so a second person can help someone play: a parent taking the hard jumps for a child, a friend steering while you handle the buttons, or a player with limited mobility sharing a game's controls with a helper. It can also turn one controller into several.

It works below the game. CtrlAssist reads the real controllers, combines what they do, and presents the result as a virtual controller. The game sees one ordinary controller and needs no support for any of this.

## What it does

- **Priority:** two controllers as one. The primary plays; whatever the assist touches, the assist takes over, for as long as it is touched.
- **Average:** two controllers as one, with the sticks and triggers blended.
- **Toggle:** two controllers as one, one at a time, handed over by a button or from the tray.
- **Unicast and multicast:** one controller as several virtual ones, one at a time or all at once.
- **Rumble** from the game reaches the real controllers, and you choose which.
- **The real controllers are hidden** from the game while this runs, so it doesn't see the same press twice.

## How you use it

A small daemon runs in your session and does the work. You tell it what to do from the **tray**, where a profile that is always there pairs two controllers in a few clicks, or from the **`ctrlassist` command**. Arrangements you want to keep are **profiles**: short files that say which controllers play which part.

Start with [Installation](./getting-started/installation.md), then the [Quick Start](./getting-started/quick-start.md).

## Words used in this guide

| Word | Means |
|---|---|
| pad | A physical game controller. |
| virtual pad | The controller CtrlAssist presents to games. |
| primary, assist | In a two-pad arrangement, the main player's pad and the helper's. |
| profile | A kept description of an arrangement: which pads, combined how. |
| setup | A profile that is running. |
| hiding | Keeping a setup's real pads out of the game's sight. |
