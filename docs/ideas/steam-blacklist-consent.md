# A way to keep CtrlAssist out of Steam's config

When a setup's pads can only be grabbed, the daemon adds their models to `controller_blacklist` in Steam's `config.vdf` and takes them out again when the setup stops. A switch would let a user say CtrlAssist may never edit that file: as a daemon setting the window shows, or as a flag on `ctrlassist service daemon` set in the unit or the NixOS module.

**Why deferred.** It is the only setting the daemon would have, and a settings file with an rpc, published state and a reload path is a lot to carry for one switch. Nobody has asked to turn it off, and a profile that wants less than hiding says so with `hide`, which already leaves Steam's file alone.

**Would bring it in.** A report of the blacklist entry causing trouble, such as a pad Steam should have seen staying hidden, or a second daemon setting that makes the file worth having.
