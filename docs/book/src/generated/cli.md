**Command Overview:**

* [`ctrlassist`↴](#ctrlassist)
* [`ctrlassist status`↴](#ctrlassist-status)
* [`ctrlassist pads`↴](#ctrlassist-pads)
* [`ctrlassist identities`↴](#ctrlassist-identities)
* [`ctrlassist identify`↴](#ctrlassist-identify)
* [`ctrlassist forget`↴](#ctrlassist-forget)
* [`ctrlassist start`↴](#ctrlassist-start)
* [`ctrlassist stop`↴](#ctrlassist-stop)
* [`ctrlassist update`↴](#ctrlassist-update)
* [`ctrlassist select`↴](#ctrlassist-select)
* [`ctrlassist reload`↴](#ctrlassist-reload)
* [`ctrlassist setups`↴](#ctrlassist-setups)
* [`ctrlassist profile`↴](#ctrlassist-profile)
* [`ctrlassist profile list`↴](#ctrlassist-profile-list)
* [`ctrlassist profile show`↴](#ctrlassist-profile-show)
* [`ctrlassist profile validate`↴](#ctrlassist-profile-validate)
* [`ctrlassist profile put`↴](#ctrlassist-profile-put)
* [`ctrlassist profile delete`↴](#ctrlassist-profile-delete)
* [`ctrlassist service`↴](#ctrlassist-service)
* [`ctrlassist service daemon`↴](#ctrlassist-service-daemon)
* [`ctrlassist service tray`↴](#ctrlassist-service-tray)

## `ctrlassist`

The ctrlassist command: the daemon as a service verb, and the clients that talk to it

**Usage:** `ctrlassist [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `status` — Say whether the daemon is running and which release it is
* `pads` — List the pads the daemon knows: those connected, and those it remembers
* `identities` — List what a virtual pad can present itself as
* `identify` — Buzz a pad, to tell which one it is
* `forget` — Stop remembering a pad that isn't connected
* `start` — Run a kept profile: its pads become its virtual pads
* `stop` — Stop a setup: its virtual pads go
* `update` — Have a running setup take up its profile as it is kept now
* `select` — Have a node of a setup select another input or output
* `reload` — Have the daemon read its files again, for edits made by hand, and bring the running setups up to them
* `setups` — List the setups that exist, with their pads and virtual pads
* `profile` — List, show, check, keep and delete profiles
* `service` — Become one of CtrlAssist's long-running processes

###### **Options:**

* `--socket <SOCKET>` — The daemon's socket, in place of the one in the user's runtime directory



## `ctrlassist status`

Say whether the daemon is running and which release it is

**Usage:** `ctrlassist status [OPTIONS]`

###### **Options:**

* `--json` — Print one JSON object, and exit with status 1 if the daemon is missing or incompatible



## `ctrlassist pads`

List the pads the daemon knows: those connected, and those it remembers

**Usage:** `ctrlassist pads [OPTIONS]`

###### **Options:**

* `--json` — Print a JSON array, one object per pad



## `ctrlassist identities`

List what a virtual pad can present itself as

**Usage:** `ctrlassist identities [OPTIONS]`

###### **Options:**

* `--json` — Print a JSON array, one object per identity



## `ctrlassist identify`

Buzz a pad, to tell which one it is

**Usage:** `ctrlassist identify [OPTIONS] <PAD>`

###### **Arguments:**

* `<PAD>` — The pad: its id as `pads` prints it, a bare address, or `model:<vendor>:<product>` when one known pad is of that model. Or a running profile's virtual pad, as `virtual:<profile>/<name>`, which is rumbled as a game would rumble it: the pads behind it buzz as the profile sends it on

###### **Options:**

* `--millis <MS>` — How long to buzz, in milliseconds

  Default value: `600`



## `ctrlassist forget`

Stop remembering a pad that isn't connected

**Usage:** `ctrlassist forget <PAD>`

###### **Arguments:**

* `<PAD>` — The pad, named as for `identify`



## `ctrlassist start`

Run a kept profile: its pads become its virtual pads

**Usage:** `ctrlassist start <PROFILE>`

###### **Arguments:**

* `<PROFILE>` — The profile's name



## `ctrlassist stop`

Stop a setup: its virtual pads go

**Usage:** `ctrlassist stop <SETUP>`

###### **Arguments:**

* `<SETUP>` — The setup's name: the profile it runs



## `ctrlassist update`

Have a running setup take up its profile as it is kept now

**Usage:** `ctrlassist update [OPTIONS] <SETUP>`

###### **Arguments:**

* `<SETUP>` — The setup's name

###### **Options:**

* `--dry-run` — Only say what the update would cost; change nothing



## `ctrlassist select`

Have a node of a setup select another input or output

**Usage:** `ctrlassist select <SETUP> <NODE> [CHOICE]`

###### **Arguments:**

* `<SETUP>` — The setup's name
* `<NODE>` — The node's name in the profile; `setups` lists the ones that select
* `<CHOICE>` — What to select: one of the node's choices as `setups` lists them, or its place among them counted from 0. Left out, the node moves to its next choice, as its switch button would



## `ctrlassist reload`

Have the daemon read its files again, for edits made by hand, and bring the running setups up to them

**Usage:** `ctrlassist reload`



## `ctrlassist setups`

List the setups that exist, with their pads and virtual pads

**Usage:** `ctrlassist setups [OPTIONS]`

###### **Options:**

* `--json` — Print a JSON array, one object per setup



## `ctrlassist profile`

List, show, check, keep and delete profiles

**Usage:** `ctrlassist profile <COMMAND>`

###### **Subcommands:**

* `list` — List the profiles the daemon keeps
* `show` — Print a profile as the TOML it was written as
* `validate` — Check a profile file, and say where it is wrong. Nothing is kept
* `put` — Have the daemon keep a profile file: a new profile, or in place of the one that has the name
* `delete` — Have the daemon stop keeping a profile



## `ctrlassist profile list`

List the profiles the daemon keeps

**Usage:** `ctrlassist profile list [OPTIONS]`

###### **Options:**

* `--json` — Print a JSON array, one object per profile



## `ctrlassist profile show`

Print a profile as the TOML it was written as

**Usage:** `ctrlassist profile show <NAME>`

###### **Arguments:**

* `<NAME>` — The profile's name



## `ctrlassist profile validate`

Check a profile file, and say where it is wrong. Nothing is kept

**Usage:** `ctrlassist profile validate <FILE>`

###### **Arguments:**

* `<FILE>` — The profile, as a TOML file



## `ctrlassist profile put`

Have the daemon keep a profile file: a new profile, or in place of the one that has the name

**Usage:** `ctrlassist profile put [OPTIONS] <FILE>`

###### **Arguments:**

* `<FILE>` — The profile, as a TOML file

###### **Options:**

* `--name <NAME>` — The profile's name, in place of the file's name without its `.toml`: lowercase letters, digits and hyphens



## `ctrlassist profile delete`

Have the daemon stop keeping a profile

**Usage:** `ctrlassist profile delete <NAME>`

###### **Arguments:**

* `<NAME>` — The profile's name



## `ctrlassist service`

Become one of CtrlAssist's long-running processes

**Usage:** `ctrlassist service <COMMAND>`

###### **Subcommands:**

* `daemon` — The daemon: owns the pads and virtual pads, and serves the clients
* `tray` — The system tray: what is running, start and stop, each profile's settings, and notifications



## `ctrlassist service daemon`

The daemon: owns the pads and virtual pads, and serves the clients

**Usage:** `ctrlassist service daemon [OPTIONS]`

###### **Options:**

* `--socket <SOCKET>` — The socket to listen on, in place of the one in the user's runtime directory
* `--state-dir <DIRECTORY>` — The directory for what the daemon remembers between runs, in place of the one in the user's state directory
* `--config-dir <DIRECTORY>` — The directory the profiles are kept under, in place of the one in the user's configuration directory
* `--hider-socket <SOCKET>` — The hiding service's socket, in place of the installed one



## `ctrlassist service tray`

The system tray: what is running, start and stop, each profile's settings, and notifications

**Usage:** `ctrlassist service tray [OPTIONS]`

###### **Options:**

* `--socket <SOCKET>` — The daemon's socket, in place of the one in the user's runtime directory
* `--config <CONFIG>` — The tray's own file, which keeps its notification switches, in place of `tray.toml` beside the daemon's files
