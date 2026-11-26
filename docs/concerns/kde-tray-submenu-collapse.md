# KDE's tray may collapse a nested submenu the first time it is opened

The tray's menu nests: a profile's submenu holds a submenu for each of its fields. On KDE Plasma, a submenu inside a submenu can close the whole outer submenu the first time it is opened after the tray starts, then work for the rest of the session. TisVlay's tray has the same shape and records the bug, its cause in Qt and KDE's `libdbusmenuqt`, the upstream reports and how to tell it from a fault of the tray's own, in its [`docs/bugs/upstream-kde-tray-submenu-collapse.md`](https://github.com/TisVlay/TisVlay/blob/main/docs/bugs/upstream-kde-tray-submenu-collapse.md).

**What is known.** Nothing observed here. CtrlAssist's tray was read and clicked over D-Bus and by hand on Plasma while it was built, and nobody looked for this; a menu that closes once and then works is easy to take for a slip of the pointer.

**Risk.** Low. The menu opens at the second try, and nothing is changed by the first.

**Mitigation.** None. The tray rebuilds its menu only when its structure changes, as TisVlay's does, since on KDE a layout update closes any open submenu, which would make the same symptom the tray's own doing.

**Closes when.** It is looked for on a current Plasma: start the tray, open a profile's submenu and hover one of its fields. If the submenu stays open, the concern closes with the Plasma version noted; if it closes, this becomes a note of the upstream bug, after TisVlay's.
