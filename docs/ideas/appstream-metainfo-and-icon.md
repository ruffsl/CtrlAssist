# AppStream metainfo and an icon of its own

The 0.4.0 release shipped a `metainfo.xml`, which is what a software center shows of an application, and the rewrite's packages ship none. The settings window's desktop entry uses the theme's `input-gaming` icon, so CtrlAssist has no mark of its own in a launcher or a task switcher.

**Why deferred.** Neither is needed to install or run from a package or from Nix. The metainfo wants a description, screenshots and a release list worth reading, and an icon wants drawing.

**Would bring it in.** A [Flatpak](flatpak.md), since Flathub requires both; a distribution's packaging asking for them; or an icon being offered.
