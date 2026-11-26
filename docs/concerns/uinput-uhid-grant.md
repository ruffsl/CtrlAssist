# The seat user is given uinput and uhid

The daemon runs as the user and makes virtual pads through `/dev/uinput`, and through `/dev/uhid` for a virtual DualSense, which a real kernel driver must bind. The rule the packages ship gives both to the user in front of the seat. Neither is limited to pads: any program of the user's can make a keyboard through the first, and feed a made-up HID device to any HID driver through the second.

**Risk.** Medium on a machine several people use, low on one person's. Access is taken away when the user switches away, but a descriptor already open stays good, so one user could keep `/dev/uinput` open, switch away, and type into the next user's session. And uhid puts every HID driver's report parser, where kernel faults have been found before, within reach of the user with no device plugged in. Within one user's own session neither adds much to what their programs can do already. Steam's rules give the same access to uinput; uhid is this project's own addition. The logind and kernel behavior here is as the reviewing agent knew it, not as observed.

**Mitigation.** None in place.

**Closes when.** The user needs neither grant, virtual pads being made by a service that hands the daemon what it writes to. The step that closes most of it for least is uhid alone: [`../ideas/uhid-behind-the-service.md`](../ideas/uhid-behind-the-service.md).
