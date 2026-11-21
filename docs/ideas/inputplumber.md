# Cooperating with InputPlumber

InputPlumber hides pads and creates virtual ones itself, as root, and is preinstalled on some handheld distributions. The two shouldn't manage the same pad. It is GPL-3.0-or-later, so it can be talked to over D-Bus but not copied from.

**Why deferred.** It couldn't be tested on the development machine, and would be a second code path.

**Would bring it in.** Someone on Bazzite who can test.
