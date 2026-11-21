# Vendored crates

Third-party crates carried in the repository because they need a fix that upstream doesn't have yet. Each is wired in through `[patch.crates-io]` in the root `Cargo.toml`, keeps its own license files, and is left out of this workspace's lints and formatting so that its diff against upstream stays the fix alone.

A crate leaves this directory when a release with its fix is published.

| Crate | Upstream | Fix |
|---|---|---|
| `evdev` 0.13.2 | [emberian/evdev](https://github.com/emberian/evdev) | `VirtualDeviceBuilder::with_phys` always failed with `EINVAL`: `UI_SET_PHYS` was declared with the size of a character, where the kernel's `_IOW('U', 108, char *)` has the size of a pointer. One declaration in `src/sys.rs`. Not yet reported upstream; it soaks here first. |

To see a vendored crate's changes, compare it with the published source: `diff -ru ~/.cargo/registry/src/*/evdev-0.13.2/src vendor/evdev/src`.

What was left out of each copy: examples, tests, and the development dependencies they needed.
