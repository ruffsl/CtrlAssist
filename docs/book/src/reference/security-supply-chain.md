# Security & Supply Chain

## What runs with privilege

Nothing runs as root. One piece is a system service rather than yours: the hiding service, `ctrlassist-hider`, which runs as a user of its own, also named `ctrlassist-hider`, with no special capabilities. Everything else runs as you: the daemon, the tray, the settings window and the command.

The hiding service is kept small so it can be read in one sitting, and is closed in on purpose:

- What it can do at all is what a udev rule installed with it gives its user's group: open a controller it has hidden, ask the system to reset a game controller's driver, and mark a controller to be hidden. It writes no system rule, and it can do nothing to a keyboard or a mouse.
- It starts only when first asked, from a socket; nothing of it runs before that.
- It serves only the person at the machine: a graphical session at the seat, not SSH or another user, and only for controllers on that seat.
- It acts only on game controllers the caller could already open, proved by handing it the open controller, never a name or a path. A handle that names a controller without opening it is refused.
- One thing it grants that you may not have had: raw access to a PlayStation controller of yours, which a virtual controller needs to be able to pass as one. It opens that for no other kind of device.
- No text from a device reaches a file name unchecked.
- It is confined by its service unit as well: no capabilities, no new privileges, no home directories, a read-only system apart from its own directory and the devices' files its rule gives it, and no device but controllers.
- One user's programs can't stop it serving another: each connection is served on its own, and one user may hold only a few.
- Whatever it hid is restored when its client goes away, however that happens, and when the service itself stops.

What this protects is the rest of the system and other users from a program running as you. It does not protect you from your own programs: a game running as you could already read your controllers. If the hiding service itself were taken over, the most it could do is what its user's group may: read the game controllers it hid; read and write the raw interface of any PlayStation controller, which takes the output and feature reports that set its lights, its rumble and its settings; reset any game controller's driver, or send any game controller's device a made-up event, which the system's own rules then act on as on a real one; and hide game controllers. That would disrupt play, not hand over the machine.

Two grants are made to you, the person at the machine, by a udev rule the packages install, as your desktop grants you your controllers themselves. One is `/dev/uinput` and `/dev/uhid`, which virtual controllers are made through. The other is the raw interface of a Steam Controller (2026), which CtrlAssist reads it through: with it, any program of yours can read everything that controller reports and change its settings, as Steam's own rules allow already wherever Steam is installed. It reaches no other device of Valve's.


## Checking a download

Each release's packages are built by the project's public workflow and signed through Sigstore, so you can check that a file was built from this repository at the tag it claims.

With the [GitHub CLI](https://cli.github.com/):

```sh
gh attestation verify ctrlassist_*.deb \
    --repo CtrlAssist/CtrlAssist \
    --signer-workflow CtrlAssist/CtrlAssist/.github/workflows/build-packages.yml
```

The same works for the `.rpm`, the Arch package, and the `ctrlassist`, `ctrlassist-hider` and `ctrlassist-gui` binaries inside them. Each release also carries `checksums-sha256.txt`:

```sh
sha256sum --ignore-missing -c checksums-sha256.txt
```

## What a package is built from

Each binary has a software bill of materials among the release's files, `ctrlassist-<version>-<arch>.cdx.json`, `ctrlassist-hider-<version>-<arch>.cdx.json` and `ctrlassist-gui-<version>-<arch>.cdx.json`, in CycloneDX format, itself signed. The binaries also carry their dependency list inside them, which tools such as `cargo audit bin` read.

The licenses of everything built in are installed with the package, in `/usr/share/doc/ctrlassist/THIRD-PARTY-LICENSES.html`.

## Reporting a problem

Please report a security problem privately, through the repository's **Security** tab on GitHub ("Report a vulnerability"), not as a public issue.
