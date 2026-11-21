# The hiding service has met fake pads only

The hiding service is built and passes its virtual machine check: hide, reconnect, restore, a killed client, its own death, and the refusals. Every pad in that check is a fake one, on drivers the kernel gives fake pads. The service has not yet hidden a real pad from a running Steam or an SDL game, and its root code has not been read by the maintainer.

**Risk.** Medium: every step after it assumes pads arrive as descriptors. The mechanism itself was run by hand on three real pads, so what is unproven is this code doing it, not the idea.

**Mitigation.** `just hider-dev` and `just hide-pad <node>` run the real service against a real pad; see [testing](../testing.md#hardware-checks).

**Closes when.** A DualSense hidden through the service disappears from Steam and SDL and returns on restore, and the root code has been read.
