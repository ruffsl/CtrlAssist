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

Not tried: Debian, Fedora or Arch; a Flatpak; the portal.

## Why, from the sources

Read afterwards in systemd 262 and the pinned nixpkgs, to explain two results that looked odd.

**`LimitRTPRIO=` in a user unit changing nothing is a documented limitation, not a bug.** A user's service manager is unprivileged, so when it applies a unit's limits and the kernel refuses, systemd lowers the request to the hard limit the manager itself has (`setrlimit_closest` in `src/basic/rlimit-util.c`), and if that leaves nothing to change it returns without a word, not even a debug line. A unit with no such line inherits the manager's limit when it is forked. `systemd.exec(5)` says as much: in a user service these settings "cannot be used to raise the limits above those set for the user manager itself", and are "only useful to lower the limits passed in or to raise the soft limit to the maximum of the hard limit"; it points to PAM or to limits on `user@.service` for raising them. So in a user unit the line can cap a service below what the manager has, or pin it, and can never grant.

**The PAM limit reaching a user service is how NixOS is built, and the note in its option is out of date.** The user manager runs under the PAM service `systemd-user`, which NixOS generates itself; every PAM service takes `security.pam.loginLimits` by default, which adds `pam_limits` to its session stack. The manager starts with that limit and its services inherit it. The note that these limits "do not apply to systemd services" was added in 2018 about system services, where it is true.

**Elsewhere:** systemd's own `systemd-user` PAM fragment has not called `pam_limits` since version 232, but Debian adds it directly, and Fedora and Arch through the `system-auth` they include. So a limits file for a group should reach user services on all three. That is read from their packaging, not measured.
