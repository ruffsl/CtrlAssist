# Spike: hiding, permissions and identification

Throwaway code for the plan's hiding spike. It answers: can an unprivileged daemon make a pad absent to games, what can a seat user open on a clean machine, and which identifiers name a pad stably.

- `src/main.rs` builds `probe`, one subcommand per observation or fixture: `access`, `pads`, `fake-pad`, `fake-hid`, `grab`, `listen`.
- `vm.nix` runs the probes in a clean NixOS VM (`just spike-hiding-vm`) and prints what it sees.

The results are in `docs/architecture.md`, under "What the platform does". This directory is deleted when the device crate replaces it.
