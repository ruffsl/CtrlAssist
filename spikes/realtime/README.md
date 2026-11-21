# Spike: letting a user's service have real-time scheduling

The daemon runs as a systemd user service, and its input thread asks for first-in-first-out scheduling at priority 10. A stock machine refuses. This spike asks which of three ways of allowing it work for a user service, and what each costs. It was run on 2026-11-21 against the nixpkgs the flake pins.

Run it with `nix build --impure --expr 'import ./spikes/realtime/test.nix { }' -L`; each result is a line starting `FINDING`. It boots five NixOS machines that differ only in the mechanism. `ask.py` is the client that asks rtkit.

## What was found

| Machine | In a user service | In a login shell | The user's service manager |
|---|---|---|---|
| Stock | limit 0, refused | limit 0 | limit 0 |
| Drop-in: `LimitRTPRIO=10` on `user@.service` | limit 10, granted | limit 0 | limit 10 |
| PAM: `security.pam.loginLimits`, rtprio 10 for the user | limit 10, granted | limit 10 | limit 10 |

- **Both the drop-in and the PAM limit work for a user service on NixOS.** The PAM result contradicts the note in nixpkgs' own option description, which says those limits "do not apply to systemd services": NixOS applies `loginLimits` to every PAM service, the user manager's included, and its services inherit the manager's limit.
- **Whether the unit itself says `LimitRTPRIO=10` made no difference** on any machine: the service has what the manager has. The unit's line can't raise it, and where the manager has it, the line isn't needed.
- **The two differ in reach.** The drop-in raises the limit for every user's services and for nothing else: a login shell stays at 0. The PAM limit raises it for the named user or group, for everything they run, shells included.

rtkit, asked by the process itself over D-Bus for priority 10:

| Case | Outcome |
|---|---|
| User with no login session (lingering only) | refused, whatever was asked |
| Logged in, no `RLIMIT_RTTIME` set | refused |
| Logged in, `RLIMIT_RTTIME` of 200 ms or 100 ms | granted, as round-robin, not first-in-first-out |
| Logged in, `RLIMIT_RTTIME` of 300 ms | refused: its ceiling is 200 ms |
| Logged in, priority 25 | refused: its ceiling is 20 |
| Granted, then the thread spins without blocking | the process is killed (`SIGKILL`) 200 ms later |

- **rtkit insists on the CPU-time limit the architecture rejected.** A thread that has the grant and then runs for 200 ms without blocking takes the whole process with it, which for the daemon is every virtual pad. The input thread blocks in `epoll_wait` between events, which resets the count, so only a bug would trip it; but the consequence of that bug changes from a busy thread, held to 95% of one CPU by the kernel's own throttle, to a dead daemon.
- **rtkit serves only a user with a login session.** A daemon started by lingering, before anyone has logged in, is refused.
- **rtkit grants round-robin.** With one real-time thread that is the same in practice.

## What it means for the choice

- The drop-in is the narrowest thing that works: user services only, priority 10 only, no group to join, no CPU-time limit, no dependency. It is what openSUSE ships for PipeWire.
- A PAM limit for a group works on NixOS too, and is narrower in who gets it, but wider in what: everything those users run. Elsewhere it works only where the distribution's PAM setup for the user manager includes `pam_limits`, which systemd's own no longer does.
- rtkit changes nothing system-wide and is already on most desktops, at the price of the kill-on-overrun limit and of a D-Bus client in the daemon. It is also the only one of the three available inside a Flatpak, through the desktop portal.

Not tried: Debian, Fedora or Arch, where the PAM result may differ; a Flatpak; the portal.
