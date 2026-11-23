# Security & Supply Chain

## What runs with privilege

One piece runs as root: the hiding service, `ctrlassist-hider`. Everything else runs as you: the daemon, the tray, the settings window and the command.

The hiding service is kept small so it can be read in one sitting, and is closed in on purpose:

- It starts only when first asked, from a socket; nothing of it runs before that.
- It serves only the person at the machine: a graphical session at the seat, not SSH or another user.
- It acts only on game controllers the caller could already open, proved by handing it the open controller, never a name or a path.
- One thing it grants that you may not have had: raw access to a PlayStation controller of yours, which a virtual controller needs to be able to pass as one. It opens that for no other kind of device.
- No text from a device reaches a system rule unchecked.
- It is confined by its service unit: no new privileges, no home directories, and a read-only system apart from `/run`.
- Whatever it hid is restored when its client goes away, however that happens, and when the service itself stops.

What this protects is the rest of the system and other users from a program running as you. It does not protect you from your own programs: a game running as you could already read your controllers.

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
