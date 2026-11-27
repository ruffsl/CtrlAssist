# A long name slides the wrong way in a right-to-left script

**What:** `SlideText` (`crates/ctrlassist-gui/ui/widgets.slint`) shows the end of a line too long for its place by moving it left. A line in a right-to-left script starts at its right, so at rest its start is in view, cut short at the left, and moving it left would bring more of its start into view and never its end.

**How much it matters:** little today. A profile's name is lowercase letters, digits and hyphens. A pad's name is whatever its hardware says, which is Latin script for every pad seen so far. The window has no translation yet; one into Arabic or Hebrew would bring captions this reaches.

**What is being done:** nothing. Slint doesn't say which way a line of text runs, so the widget can't tell which end to move toward without being told.

**What would close it:** the widget told the direction, by the window's locale or by Slint exposing a text's direction, and sliding toward the line's end either way, with a test that holds a right-to-left line.

Found in the review of the pull request for the setups' status (#65), by reading; not tried.
