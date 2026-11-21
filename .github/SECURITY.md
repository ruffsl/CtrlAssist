# Security policy

## Supported versions

Security fixes go into the next release from `main`. Only the latest minor release is supported; earlier minors don't get backports.

That covers everything this repository ships: the `.deb`, `.rpm` and Arch packages on the releases page, and the Nix package and NixOS module. The part that runs as root, the hiding service, is described in the user guide's [Security & Supply Chain](/docs/book/src/reference/security-supply-chain.md) page, with what it is meant to defend and what it isn't.

## Reporting a vulnerability

Please don't open a public issue. Report it privately instead, through GitHub's [private vulnerability reporting](https://github.com/CtrlAssist/CtrlAssist/security/advisories/new) form: the repository's Security tab, then "Report a vulnerability".

It helps to include:

- the affected version, and how CtrlAssist was installed;
- what an attacker can do, and from where: another local user, a program running as the same user, or a device plugged into the machine;
- steps to reproduce, or a proof of concept.

## What to expect

CtrlAssist is maintained by volunteers, so reports are handled on a best-effort basis, with no fixed timeline. Replies come in the report's advisory thread, where the fix and its disclosure can be worked out together.

## Disclosure

Once a fix is released, it's published as a GitHub security advisory, which can request a CVE, crediting the reporter unless they'd rather not be named.
