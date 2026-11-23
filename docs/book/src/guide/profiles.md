# Profiles

A profile is a short TOML file that describes an arrangement: which controllers take part and how they are combined. The daemon keeps profiles by name and runs them as setups.

```sh
ctrlassist profile put couch.toml      # keep it, named `couch` after the file
ctrlassist profile put couch.toml --name sofa
ctrlassist profile list
ctrlassist profile show couch
ctrlassist profile validate couch.toml # check a file without keeping it
ctrlassist profile delete couch
```

A name is 1 to 32 lowercase letters, digits and hyphens. A profile that is wrong is refused, with where and what:

```text
Error: node.second.inputs[1]: no source or node named `tow` (invalid-profile)
```

Putting a profile doesn't change a setup that is already running. `ctrlassist update <name>` makes it take up the profile as kept now; see [below](#changing-a-running-setup).

## Presets

A preset is a whole arrangement in a few lines: `mode` names one of five arrangements, and the rest fill it in.

```toml
mode = "priority"
primary = "0a:1b:2c:3d:4e:14"
assist = "model:054c:09cc"
identity = "xbox"       # optional
rumble = "primary"      # optional
hide = "grab"           # optional
managed = true       # optional
```

| Mode | Takes | Does |
|---|---|---|
| `priority` | `primary`, `assist` | Two pads as one. The assist takes over whatever it touches. |
| `average` | `primary`, `assist` | Two pads as one. Sticks and triggers blend. |
| `toggle` | `primary`, `assist` | Two pads as one, one at a time. The Mode button hands over. |
| `unicast` | `pad`, `outputs` | One pad as several virtual pads, one at a time. |
| `multicast` | `pad`, `outputs` | One pad as several virtual pads, all at once. |

`outputs` is how many virtual pads to make; 2 if left out.

`rumble` is who feels what a game rumbles, for the three presets of two controllers: `both` (the default), `primary`, `assist` or `none`. A helper who only steps in now and then may not want the rumble; a player who can't feel it may want the helper to.

`hide` is how the real controllers are kept from games while the arrangement runs, in any profile, a preset or not:

| `hide` | Games see the real controllers? |
|---|---|
| `full` (the default) | No. The hiding service takes them away; where it isn't installed they are grabbed, and Steam is told to leave them alone. |
| `grab` | Most don't. A program that reads controllers another way, as Steam does, still sees them. |
| `none` | Yes, beside the virtual one. Use it when a game should read a real controller directly, and expect a game that reads both to take a press twice. |

## Naming a controller

`ctrlassist pads` prints what each controller is called, in its `ID` column. A profile can name one three ways:

| Written as | Means |
|---|---|
| `0a:1b:2c:3d:4e:14` | That controller, by its own address. |
| the full id from `ctrlassist pads`, such as `serial:045e:0b12:3039…` | That exact controller. |
| `model:054c:0ce6` | Any controller of that vendor and product. |

A `model:` name is handy but can't tell two identical controllers apart. If two parts name the same model, each takes a different controller, in the order they were connected. When that order matters, name them exactly.

Once a part has taken a controller, it waits for that same controller if it disconnects.

## Identity

`identity` is what the virtual controller says it is to games:

| Identity | Appears as |
|---|---|
| `generic` (the default) | A plain gamepad that games map from its buttons. |
| `xbox` | An Xbox Series controller, which most games recognize outright. |
| `dualsense` | A PlayStation 5 DualSense controller, with PlayStation button prompts in games that have them. |

`ctrlassist identities` lists them and whether each can be made on this machine.

If you connect a real DualSense by USB cable, with the hiding service installed, CtrlAssist copies what that controller says of itself, keeps the copy in `~/.local/state/ctrlassist/identities/`, and makes its virtual DualSenses from that from then on; until then it uses a description of the controller that ships with it. You don't need to own one.

A virtual DualSense carries the buttons, sticks, triggers and rumble. Its touchpad and motion sensors are there and report nothing, and the system shows a battery and lights for it, as it does for a real one.

When the controller behind it is a real DualSense, on a USB cable or over Bluetooth, and the hiding service has hidden it (`hide` left at `full`, with the service installed), the game's lightbar, player lights and adaptive trigger effects reach it too. A DualShock 4 behind it, either way, gets the lightbar's color. A controller that is only grabbed, or not hidden, gets only the rumble. A controller that a game-streaming host presents as a DualSense is treated as one; whether the lights and trigger effects then reach the controller at the far end depends on the host.

## The default profile

CtrlAssist always keeps one profile, `default`: it is there from the first start, with its controllers still to choose, so there is always a profile to fill in from the tray or the window. It is listed first, with a check mark. Change it as you like; it can't be deleted or renamed, and if its file is removed it is made again.

A profile whose controllers aren't chosen yet is kept and shown as **Not set up**. It isn't wrong; it starts once its controllers are chosen. `ctrlassist profile list` says which ones those are.

## Managed profiles

A profile is started by hand unless it is managed. With `managed = true`, the daemon starts the profile whenever every controller it names is connected: when the daemon starts, when you put the profile, and when one of its controllers is plugged in. If you stop it by hand it stays stopped until one of its own controllers is plugged in again.

## The full form

A preset is shorthand. The full form names every part, for arrangements a preset can't describe. Here one helper assists two players in turn:

```toml
[source.one]
pad = "0a:1b:2c:3d:4e:14"

[source.two]
pad = "a4:5e:60:c1:22:07"

[source.helper]
pad = "1a:2b:3c:4d:5e:da"
deadzone = 0.15                    # this one's stick is worn

[node.share]
demux = "unicast"
input = "helper"
outputs = 2

[node.first]
mux = "priority"
inputs = ["one", "share.0"]        # later inputs outrank earlier ones

[node.second]
mux = "priority"
inputs = ["two", "share.1"]
engage = 0.2
release = 0.1
rules = { trigger = "blended" }    # priority everywhere else
feedback = ["two"]                 # the helper doesn't feel this game

[sink.player-one]
input = "first"
identity = "generic"

[sink.player-two]
input = "second"
identity = "xbox"
```

- A **source** is a controller. `deadzone` treats a stick resting inside it as untouched: a number from 0 to less than 1, the whole of a stick's travel, and 0.1 if it isn't written.
- A **node** combines. A `mux` takes several `inputs` to one output, by `priority`, `average` or `toggle`. A `demux` takes one `input` to several `outputs`, `unicast` or `multicast`; its outputs are called `name.0`, `name.1` and so on.
- A **sink** is a virtual controller, following a source or a node.
- `engage` and `release` are how far a stick or trigger must move for a mux to count an input as touching it, and how far back before it lets go.
- `feedback` says who feels rumble: `"active"` (whoever is playing), `"none"`, or a list of a mux's inputs.
- `switch` is the button that hands over a toggle or a unicast: a button's name, or `false` for none.

A preset and the full form don't mix in one file.

## Changing a running setup

```sh
ctrlassist profile put couch.toml
ctrlassist update couch --dry-run   # what would it cost?
ctrlassist update couch
```

An update that keeps the same virtual controllers, as many and of the same identities, is applied as the setup runs, and a game notices nothing: other controllers, a mode, a threshold, a rule, a deadzone, who feels rumble, the switch button, even how the parts are wired. A controller that is swapped out is given back to the system and the new one hidden; one that only changes parts stays as it is. A toggle goes on with whoever had control. Changing a virtual controller's identity, how many there are, or `hide`, restarts the setup, which a game sees as its controller disconnecting and a new one connecting. `update` says which it is, before and after. If the profile as kept is wrong or gone, the setup carries on as it was.

### Editing the files by hand

The daemon keeps each profile as `<name>.toml` in `~/.config/ctrlassist/profiles`, and goes by what it read when it started. After editing a file there, or adding or removing one:

```sh
ctrlassist reload
```

It says what it found changed. A running setup whose profile changed is brought up to it at once: as it runs where the virtual controllers stay the same, and by a restart where one must be made anew. A file with something wrong in it is still listed, with what is wrong, and a setup running its earlier text carries on as it was.

To hand over a toggle or a unicast without its button:

```sh
ctrlassist select couch mux assist   # by name
ctrlassist select couch mux          # the next one
```

`ctrlassist setups` shows each such node with its choices, the current one in brackets.

## Where profiles live

In `~/.config/ctrlassist/profiles/`, one file each. `profile put` is the usual way to write them. A file edited in place is read at `ctrlassist reload` or when the daemon next starts, as [above](#editing-the-files-by-hand), and one it can't understand is listed with what is wrong and left untouched.
