# Configuration Files

A profile is kept as a short TOML file. This page is the reference for what such a file can say and for the commands that keep, check and change them; [Profiles](./profiles.md) says what the choices mean.

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
assist-gyro = "left-bumper"  # optional
hide = "grab"           # optional
managed = true       # optional
```

| Mode | Takes | Does |
|---|---|---|
| `priority` | `primary`, `assist` | Two controllers as one. The assist takes over whatever it touches. |
| `average` | `primary`, `assist` | Two controllers as one. Sticks and triggers blend. |
| `toggle` | `primary`, `assist` | Two controllers as one, one at a time. The Mode button hands over. |
| `unicast` | `pad`, `outputs` | One controller as several virtual ones, one at a time. |
| `multicast` | `pad`, `outputs` | One controller as several virtual ones, all at once. |

`outputs` is how many virtual controllers to make; 2 if left out. What each mode does is on [Profiles](./profiles.md#mode).

`rumble` is who feels what a game rumbles, for the three presets of two controllers: `both` (the default), `primary`, `assist` or `none`. See [Rumble](./profiles.md#rumble).

`assist-gyro` is the assist's button that gives a game the assist's motion sensors while it is held, for the `priority` and `average` presets: a button's name (`south`, `east`, `north`, `west`, `left-bumper`, `right-bumper`, `select`, `start`, `mode`, `left-thumb`, `right-thumb`, `dpad-up`, `dpad-down`, `dpad-left`, `dpad-right`, `touchpad`, or on a [Steam Controller](./steam-controller.md#what-carries-over) its back buttons `paddle-1` to `paddle-4`, or a thumb or hand resting on it: `left-stick-touch`, `right-stick-touch`, `left-grip-touch`, `right-grip-touch`), or `none`, the default. See [Gyro](./profiles.md#gyro).

`hide` is how the real controllers are kept from games while the arrangement runs, in any profile, a preset or not: `full` (the default), `grab` or `none`, which the GUI calls Hidden, Grabbed and Visible. See [Visibility](./profiles.md#visibility).

`identity` is what the virtual controller presents itself as: `generic` (the default), `xbox`, `dualsense` or `steam-controller`; `ctrlassist identities` lists them and whether each can be made on this machine. See [Identity](./profiles.md#identity).

`managed = true` has the daemon start the profile whenever its controllers are connected. See [Managed](./profiles.md#managed).

## Naming a controller

`ctrlassist pads` prints what each controller is called, in its `ID` column. A profile can name one three ways:

| Written as | Means |
|---|---|
| `0a:1b:2c:3d:4e:14` | That controller, by its own address. |
| the full id from `ctrlassist pads`, such as `serial:045e:0b12:3039…` | That exact controller. |
| `model:054c:0ce6` | Any controller of that vendor and product. |

A `model:` name is handy but can't tell two identical controllers apart. If two parts name the same model, each takes a different controller, in the order they were connected. When that order matters, name them exactly.

Once a part has taken a controller, it waits for that same controller if it disconnects.

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
- `switch` is the button that hands over a toggle or a unicast: a button's name, the same names as `assist-gyro`'s but for the touches, or `false` for none.
- `gyro` is a mux's button that hands the motion sensors to one of its inputs while that input holds it: `{ button = "left-bumper", input = "assist" }`. A mux that selects an input, as a toggle does, has none.

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
