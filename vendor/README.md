# Vendored crates

Third-party crates carried in the repository because they need a fix that upstream doesn't have yet. Each is wired in through `[patch.crates-io]` in the root `Cargo.toml`, keeps its own license files, and is left out of this workspace's lints and formatting so that its diff against upstream stays the fix alone.

A crate leaves this directory when a release with its fix is published.

| Crate | Upstream | Fix |
|---|---|---|
| `evdev` 0.13.2 | [emberian/evdev](https://github.com/emberian/evdev) | `VirtualDeviceBuilder::with_phys` always failed with `EINVAL`: `UI_SET_PHYS` was declared with the size of a character, where the kernel's `_IOW('U', 108, char *)` has the size of a pointer. One declaration in `src/sys.rs`. Not yet reported upstream; it soaks here first. |
| | | Nothing the input thread calls allocates or duplicates a descriptor. `RawDevice` and `VirtualDevice` start with room for their event buffers (`src/raw_stream.rs`, `src/uinput.rs`), where the first read of each allocated it. `FFUploadEvent` and `FFEraseEvent` borrow the virtual device's descriptor instead of duplicating it, so they gain a lifetime: a failed `dup` began an upload or erase that was never ended, and the game asking waited 30 seconds. `RawDevice::upload_ff_effect` duplicates its descriptor before uploading, not after, so a failed `dup` leaves no effect behind. Not reported upstream. |
| | | `Device::fetch_events` kept a resync after `SYN_DROPPED` only if the read that followed it found more events: when the batch with the marker had emptied the kernel's queue, the read failed with `EAGAIN`, and the state fetched, with the events read after the marker, waited for the device's next report, so a dropped release stayed held until then. Now the resync is handed over either way (`src/sync_stream.rs`, `fetch_events_inner`). Starting with room for a whole batch made this the usual case. Not reported upstream. |

To see a vendored crate's changes, compare it with the published source: `diff -ru ~/.cargo/registry/src/*/evdev-0.13.2/src vendor/evdev/src`.

What was left out of each copy: examples, tests, and the development dependencies they needed.
