# Gyro, touchpad, mouse and keyboard

Controls that report movement rather than position have no level to diff, so the level-based model can't carry them as it stands. A uhid virtual pad could present them.

**Why deferred.** Out of scope for an assist tool's first release.

**Would bring it in.** A use for gyro or touchpad assist. One came on 2026-11-23: a hidden pad now has its touchpad and sensor nodes hidden too, so they are lost to a game while a setup runs. A PlayStation pad's are now carried to a virtual DualSense, beside the controls and outside the level-based model ([the architecture](../architecture.md#motion-and-the-touchpad)); a mouse and a keyboard stay here.
