# Concerns

Things the design depends on that have not been observed, or risks accepted for now. One file each: what the concern is, how much it matters, what is being done, and what would close it.

## Open

- [`hiding-untested-drivers.md`](hiding-untested-drivers.md) — hiding by re-bind is unobserved for pads without a HID driver.
- [`hiding-service-unbuilt.md`](hiding-service-unbuilt.md) — the hiding service is proven as commands, not as a process.
- [`pad-identity-across-transports.md`](pad-identity-across-transports.md) — a pad's identifier is unverified across transports.
- [`suspend-and-reconnect.md`](suspend-and-reconnect.md) — suspend, resume and a Bluetooth drop are untested.
- [`realtime-refused.md`](realtime-refused.md) — without real-time scheduling the latency tail is not dependable under load.
- [`identity-unverified-input.md`](identity-unverified-input.md) — virtual pad identities were detected, not played.
