# Setup

Setup Flatpak and Flathub repository:

- https://docs.flatpak.org/en/latest/first-build.html

```sh
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
```

Generate Cargo sources for Flatpak build:

- https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo

```sh
python3 ~/git/flatpak/flatpak-builder-tools/cargo/flatpak-cargo-generator.py \
    ../Cargo.lock \
    -o cargo-sources.json
```

# Build

Build and install Flatpak locally:

```shell
flatpak-builder \
    --user \
    --force-clean \
    --install-deps-from=flathub \
    --repo=repo \
    --install builddir \
    io.github.ruffsl.ctrlassist.yml
```

# Run

Run and test Flatpak build with debug logging:

```
RUST_BACKTRACE=1 RUST_LOG=debug flatpak run io.github.ruffsl.ctrlassist mux
```

# Bundle

Bundle Flatpak for distribution:

```shell
flatpak build-bundle repo \
    --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo
    ctrlassist.flatpak \
    io.github.ruffsl.ctrlassist \
```

# References

- Permissions needed for device access:
    - https://docs.flatpak.org/en/latest/sandbox-permissions.html#device-access
    - https://github.com/flatpak/flatpak/pull/5481
    - https://github.com/flatpak/flatpak/issues/5681
    - https://github.com/flatpak/flatpak/pull/6285
