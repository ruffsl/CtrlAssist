# Command Line

Everything the tray does, the `ctrlassist` command does too, which suits scripts and remote sessions.

Every command is a short request to the daemon, except `service daemon` and `service tray`, which *are* the daemon and the tray. With no daemon running a command says so at once and exits with status 1.

## Which command does what the tray does

| In the tray | On the command line |
|---|---|
| A profile's **Start** / **Stop** | `ctrlassist start <name>`, `ctrlassist stop <name>` |
| A running profile's details | `ctrlassist setups` |
| **Has control** | `ctrlassist select <setup> <node> [choice]` |
| **Quick start** | Write a profile, then `ctrlassist profile put` and `ctrlassist start` |
| A pick while `quick` runs, **Restart as `quick`** | `ctrlassist profile put`, then `ctrlassist update <name>` |
| **Pads** | `ctrlassist pads` |
| **Identify**, **Forget** | `ctrlassist identify <pad>`, `ctrlassist forget <pad>` |
| The status line | `ctrlassist status` |

## For scripts

`status`, `pads`, `identities`, `setups` and `profile list` take `--json` and print one JSON value.

```sh
ctrlassist setups --json | jq -r '.[] | "\(.name) \(.phase)"'
```

`ctrlassist status --json` exits with status 1 when the daemon is missing or runs a release this command can't talk to, after printing its JSON, with nothing on standard error.

A request the daemon refuses ends with a code in brackets that a script can match on, such as `(unknown-pad)`, `(pad-in-use)`, `(invalid-profile)` or `(setup-exists)`.

## Completions

The packages install completions for bash, zsh and fish. They complete the command's own words, and the names of your controllers, profiles and setups, read from the running daemon as you press Tab.

## Reference

Generated from the command itself.

{{#include ../generated/cli.md}}
