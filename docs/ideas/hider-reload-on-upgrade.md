# Upgrading the hiding service without showing hidden pads

The hiding service stays once started, so a package upgrade has to restart it for the new binary to run. The deb, rpm and Arch packages do, with `systemctl try-restart`, and a NixOS switch that changes the package stops the service and restarts its socket, which comes to the same. If pads are hidden just then, the service's stop shows them again by re-binding each, the daemon finds its connection dead when the pad comes back, and hides the pad again on a new one: the pad is there for games for a few seconds, and is re-bound twice.

The service could take a reload to mean "exit once the last connection closes": an `ExecReload=` that signals it, with the packages asking `try-reload-or-restart` and the module setting `reloadIfChanged`. Pads would stay hidden until their setups stop, and the new binary would run from the next start.

**Why deferred.** An upgrade in the middle of a game is rare and what it costs is brief and mended by the daemon itself. A reload would leave the old binary serving for as long as a setup runs, which is the wrong way round for a fix to the service's security; and it gives the service a signal to handle and a second way to end.

**Would bring it in.** Reports of pads showing in a game as a system updates itself in the background; or a distribution whose updates run unattended while people play.
