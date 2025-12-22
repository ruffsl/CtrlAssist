# Setup

- https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo

```shell
$ python3 ~/git/flatpak/flatpak-builder-tools/cargo/flatpak-cargo-generator.py ../Cargo.lock -o cargo-sources.json
...
```

# Build

```shell
$ flatpak-builder --force-clean --user --install-deps-from=flathub --repo=repo --install builddir io.github.ruffsl.ctrlassist.yml
...
$ RUST_BACKTRACE=1 RUST_LOG=debug flatpak run io.github.ruffsl.ctrlassist mux
...
```

# References

- https://docs.flatpak.org/en/latest/sandbox-permissions.html#device-access
