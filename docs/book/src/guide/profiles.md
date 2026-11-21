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

A preset is a whole arrangement in a few lines.

```toml
preset = "priority"
primary = "0c:27:56:4f:bd:14"
assist = "model:054c:09cc"
identity = "xbox"       # optional
auto-start = true       # optional
```

| Preset | Takes | Does |
|---|---|---|
| `priority` | `primary`, `assist` | Two pads as one. The assist takes over whatever it touches. |
| `average` | `primary`, `assist` | Two pads as one. Sticks and triggers blend. |
| `toggle` | `primary`, `assist` | Two pads as one, one at a time. The Mode button hands over. |
| `unicast` | `pad`, `outputs` | One pad as several virtual pads, one at a time. |
| `multicast` | `pad`, `outputs` | One pad as several virtual pads, all at once. |

`outputs` is how many virtual pads to make; 2 if left out.

## Naming a controller

`ctrlassist pads` prints what each controller is called, in its `ID` column. A profile can name one three ways:

| Written as | Means |
|---|---|
| `0c:27:56:4f:bd:14` | That controller, by its own address. |
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

A virtual DualSense carries the buttons, sticks, triggers and rumble. When the controller behind it is a real DualSense on a USB cable, with the hiding service installed, the game's lightbar, player lights and adaptive trigger effects reach it too. Its touchpad and motion sensors are there and report nothing, and the system shows a battery and lights for it, as it does for a real one.

## Starting by itself

With `auto-start = true`, the daemon starts the profile whenever every controller it names is connected: when the daemon starts, when you put the profile, and when one of its controllers is plugged in. If you stop it by hand it stays stopped until one of its own controllers is plugged in again.

## The full form

A preset is shorthand. The full form names every part, for arrangements a preset can't describe. Here one helper assists two players in turn:

```toml
[source.one]
pad = "0c:27:56:4f:bd:14"

[source.two]
pad = "a4:5e:60:c1:22:07"

[source.helper]
pad = "1c:96:5a:a1:35:da"
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

- A **source** is a controller. `deadzone` treats a stick resting inside it as untouched.
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

An update that keeps the same virtual controllers, as many and of the same identities, is applied as the setup runs, and a game notices nothing: other controllers, a mode, a threshold, a rule, a deadzone, who feels rumble, the switch button, even how the parts are wired. A controller that is swapped out is given back to the system and the new one hidden; one that only changes parts stays as it is. A toggle goes on with whoever had control. Changing a virtual controller's identity, or how many there are, restarts the setup, which a game sees as its controller disconnecting and a new one connecting. `update` says which it is, before and after. If the profile as kept is wrong or gone, the setup carries on as it was.

To hand over a toggle or a unicast without its button:

```sh
ctrlassist select couch mux assist   # by name
ctrlassist select couch mux          # the next one
```

`ctrlassist setups` shows each such node with its choices, the current one in brackets.

## Where profiles live

In `~/.config/ctrlassist/profiles/`, one file each. They are the daemon's: write them with `profile put`, not by editing there. A file edited in place is read only when the daemon next starts, and one it can't understand is listed with what is wrong and left untouched.
