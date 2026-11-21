# CtrlAssist project tasks
#
# Run `just` (or `just --list`) to see all recipes, grouped by purpose.
# This justfile is the canonical source of truth for project commands:
# prek hooks, editor tasks and CI workflows all dispatch through here.
# A recipe is added in the commit that first gives it something to check.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Default recipe: list all available recipes, grouped.
default:
    @just --list --unsorted

# --- Quality checks (prek hooks) ----------------------------------------

# Shared build + clippy scope, kept in one place so the two recipes can't
# drift: test profile, whole workspace, all targets + all features.
check_flags := "--profile test --workspace --all-targets --all-features"

# The gates deny warnings through cargo's `build.warnings` rather than
# `-- -D warnings`: it covers clippy, test and rustdoc builds alike, fails
# replayed (cached) warnings too, and toggling it doesn't invalidate the
# build cache. It is set per gate recipe, never in `.cargo/config.toml`,
# which the Nix package build reads with nixpkgs' compiler. Local `cargo
# build` and `cargo run` stay warn-only.
deny_warnings := "CARGO_BUILD_WARNINGS=deny"

# Format all Rust (rustfmt) and TOML (tombi; config in .config/tombi.toml)
[group('check')]
fmt:
    cargo fmt
    tombi format --quiet

# Build the workspace (test profile, all targets, all features)
[group('check')]
build:
    cargo build {{ check_flags }}

# Run clippy with warnings denied
[group('check')]
clippy:
    {{ deny_warnings }} cargo clippy {{ check_flags }}

# `--no-deps`: only the workspace's own docs are checked. `--profile test`
# reuses the dependency metadata `just clippy` built.
#
# Build the API docs with warnings denied
[group('check')]
doc:
    {{ deny_warnings }} cargo doc --profile test --workspace --no-deps --all-features

# cargo-shear parses the sources (no build), so it runs in seconds; false
# positives go under a crate's `[package.metadata.cargo-shear] ignored`,
# with a comment saying why.
#
# Find unused or misplaced dependencies
[group('check')]
shear:
    cargo shear --deny-warnings

# `-D unnecessary-skip -D unmatched-skip` make a stale skip-list entry an
# error too, so the list can't outlive the duplicates it excuses.
#
# Check licenses, one version per crate, and that everything comes from crates.io
[group('check')]
deny:
    cargo deny --config packaging/licensing/deny.toml check \
        -D unnecessary-skip -D unmatched-skip licenses bans sources

# typos doesn't search `.config/` yet, so the config path is explicit.
#
# Spell-check code, docs and identifiers, enforcing American English
[group('check')]
typos:
    typos --config .config/typos.toml

# nextest runs each test in its own process; doc tests need cargo itself.
#
# Run the tests (nextest, then doc tests)
[group('check')]
test:
    cargo nextest run --workspace --all-features
    cargo test --workspace --all-features --doc

# Lint the GitHub workflows: syntax, expressions, and their `run:` lines
[group('check')]
actionlint:
    actionlint

# Without a GitHub token in GH_TOKEN it runs offline and says so.
#
# Audit the workflows and the Dependabot config for security problems
[group('check')]
zizmor:
    zizmor .github/

# Run every local gate
[group('check')]
check: deny shear typos actionlint zizmor fmt build clippy doc test

# --- CI ------------------------------------------------------------------
#
# What the workflows run, so that a CI failure reproduces locally.

# The line coverage the workspace must keep. Raise it with `just cov` when
# coverage rises; never lower it.
cov_baseline_lines := "95"

# Kept out of the coverage figure: test infrastructure; the hiding
# service's own files, which need root and real device nodes, and are
# exercised by the virtual machine checks, its client and wire format
# being counted; and the two files of the tray that need a desktop's
# session bus, which hold only what puts the rest on a desktop.
cov_exclude := "--ignore-filename-regex 'ctrlassist-test/|ctrlassist-hider/src/(main|activation|seat|pad|hide)\\.rs|ctrlassist-tray/src/(desktop|service)\\.rs'"

# Advisories and yanked crates, which change without the lockfile changing.
#
# Check the dependencies against the RustSec advisory database
[group('ci')]
audit:
    cargo deny --config packaging/licensing/deny.toml check advisories

# Print a coverage summary
[group('ci')]
cov:
    cargo llvm-cov nextest --workspace --all-features {{ cov_exclude }} --summary-only

# One instrumented build serves clippy, the tests and the report:
# `show-env` routes every later cargo command through cargo-llvm-cov, so
# the stages share a target directory instead of compiling twice.
#
# What CI runs: clippy, nextest and doc tests under coverage, then the floor
[group('ci')]
test-ci:
    #!/usr/bin/env bash
    set -euo pipefail
    export CARGO_PROFILE_DEV_DEBUG=line-tables-only
    export {{ deny_warnings }}
    export CARGO_TARGET_DIR=target/llvm-cov-target
    eval "$(cargo llvm-cov show-env --sh)"
    cargo llvm-cov clean --workspace
    cargo clippy --workspace --all-targets --all-features
    cargo nextest run --workspace --all-features
    cargo test --workspace --all-features --doc
    cargo llvm-cov report {{ cov_exclude }} --lcov --output-path lcov.info
    cargo llvm-cov report {{ cov_exclude }} \
        --fail-under-lines {{ cov_baseline_lines }} --summary-only

# The flake's `msrv-rust` toolchain (read from Cargo.toml's `rust-version`)
# goes first on PATH. No `--ignore-rust-version`: the toolchain is the
# floor, so a dependency that outgrows it fails here.
#
# Check the workspace on its oldest supported compiler
[group('ci')]
msrv:
    #!/usr/bin/env bash
    set -euo pipefail
    tc=$(nix build --no-link --print-out-paths .#msrv-rust)
    PATH="$tc/bin:$PATH" cargo check --workspace --all-targets --all-features --target-dir target/msrv

# The checks are in packaging/nix/test-*.nix. Each boots a NixOS virtual
# machine, so they need KVM and take a minute or two.
#
# Run one virtual machine check, such as `just vm platform`
[group('ci')]
vm name:
    nix build .#checks.x86_64-linux.{{ name }} --print-build-logs

# --- Dev -----------------------------------------------------------------
#
# Run the stack from the build tree. The socket is the installed daemon's
# own unless CTRLASSIST_SOCKET names another, so a dev daemon and an
# installed one don't run side by side by accident.

runtime_dir := env("XDG_RUNTIME_DIR", "/tmp")
socket := env("CTRLASSIST_SOCKET", runtime_dir / "ctrlassist/zenoh.sock")

# What the dev daemon remembers and keeps is apart from an installed daemon's.
state_dir := env("CTRLASSIST_STATE_DIR", justfile_directory() / "target/dev-state")
config_dir := env("CTRLASSIST_CONFIG_DIR", justfile_directory() / "target/dev-config")

# RELEASE=true runs optimized builds.
_dev_profile := if env("RELEASE", "false") == "true" { "release" } else { "test" }
_daemon_log := "info,ctrlassist_daemon=debug,ctrlassist_protocol=debug,zenoh=warn"

# With `just hider-dev` running in another terminal, a setup's pads are
# hidden through it; without, they are grabbed.
#
# Start the daemon
[group('dev')]
daemon-start:
    RUST_LOG="{{ _daemon_log }}" \
        cargo run --profile {{ _dev_profile }} --bin ctrlassist -- service daemon \
        --socket "{{ socket }}" --state-dir "{{ state_dir }}" --config-dir "{{ config_dir }}" \
        --hider-socket "{{ hider_dev_socket }}"

# Start the tray against the dev daemon
[group('dev')]
tray-start:
    cargo run --profile {{ _dev_profile }} --bin ctrlassist -- service tray --socket "{{ socket }}"

# The hiding service from `just hider-dev` runs as root and is left
# alone: Ctrl-C in its own terminal stops it.
#
# Kill the dev daemon and tray
[group('dev')]
stop-all:
    # Matched on the dev build's path, so an installed daemon keeps running.
    -pkill -f 'target/(debug|release)/ctrlassist service (daemon|tray)'
    # SIGTERM returns at once; wait until the processes are gone so a
    # start that follows can't race the exit. The daemon stops its setups
    # and restores their pads on the way out. Five seconds at most, so a
    # stuck process can't wedge the recipe.
    @timeout 5 sh -c 'while pgrep -f "target/(debug|release)/ctrlassist service (daemon|tray)" > /dev/null; do sleep 0.1; done' || true

# Run the CLI against the dev daemon, such as `just cli status`
[group('dev')]
cli *args:
    cargo run --quiet --profile {{ _dev_profile }} --bin ctrlassist -- --socket "{{ socket }}" {{ args }}

# --- Maintenance ---------------------------------------------------------

controller_db := "crates/ctrlassist-device/data"
controller_db_repo := "https://github.com/mdqinc/SDL_GameControllerDB"

# Only the Linux entries are kept, under a header naming the commit. Review
# the diff before committing: every line is input to the daemon.
#
# Refresh the pinned controller database snapshot from upstream
[group('maintain')]
update-controller-db:
    #!/usr/bin/env bash
    set -euo pipefail
    sha=$(git ls-remote {{ controller_db_repo }} HEAD | cut -f1)
    raw="https://raw.githubusercontent.com/mdqinc/SDL_GameControllerDB/$sha"
    curl -fsS "$raw/LICENSE" -o {{ controller_db }}/LICENSE-gamecontrollerdb
    {
        echo "# SDL_GameControllerDB, Linux entries only."
        echo "# Source: {{ controller_db_repo }}"
        echo "# Commit: $sha"
        echo "# License: zlib; see LICENSE-gamecontrollerdb beside this file."
        echo "# Refreshed by \`just update-controller-db\`; not edited by hand."
        curl -fsS "$raw/gamecontrollerdb.txt" | grep "platform:Linux"
    } > {{ controller_db }}/gamecontrollerdb.txt
    git diff --stat -- {{ controller_db }}

# --- Hardware -----------------------------------------------------------

# Checks on real pads, which no virtual machine has. The service runs from
# the build tree on a socket of its own, beside any installed one.

hider_dev_socket := "/run/ctrlassist-dev.sock"

# Asks for sudo. Leave it running and use `just hide-pad` in another
# terminal; Ctrl-C stops it and restores whatever is hidden.
#
# Run the hiding service from the build tree, as root
[group('hardware')]
hider-dev:
    cargo build --quiet --package ctrlassist-hider
    sudo target/debug/ctrlassist-hider --listen {{ hider_dev_socket }}

# Pass the pad's event node, such as /dev/input/event30;
# `ctrlassist-hider check <node>` says what the service makes of one.
# Ctrl-C closes the connection, which restores the pad.
#
# Hide a real pad through `just hider-dev` and hold it hidden
[group('hardware')]
hide-pad node:
    cargo build --quiet --package ctrlassist-test
    target/debug/ctrlassist-fixture hide {{ node }} --socket {{ hider_dev_socket }} --control target/hide-pad

# Two fake pads through the input thread's mux to a virtual pad, at 1,000
# events a second each. Compare with the table in docs/architecture.md.
#
# Time the input path, idle and with every CPU busy
[group('hardware')]
bench seconds="10":
    cargo build --quiet --release --package ctrlassist-test
    target/release/ctrlassist-bench --seconds {{ seconds }}
    target/release/ctrlassist-bench --seconds {{ seconds }} --load "$(nproc)"

# The same measurement with the input thread inside a daemon, which the
# bench runs as a child with a profile over its two fake pads: what the
# control plane beside the input thread costs it.
#
# Time the input path through the daemon, idle and with every CPU busy
[group('hardware')]
bench-daemon seconds="10":
    cargo build --quiet --release --package ctrlassist-test --package ctrlassist-cli
    target/release/ctrlassist-bench --seconds {{ seconds }} --daemon target/release/ctrlassist
    target/release/ctrlassist-bench --seconds {{ seconds }} --daemon target/release/ctrlassist --load "$(nproc)"

# Pass the pad's event node, and `--xbox` for the Xbox identity. Needs
# uinput, which the packaged udev rule or Steam's own gives the seat user.
# Ctrl-C removes the virtual pad.
#
# Put a virtual pad in front of a real pad
[group('hardware')]
forward-pad node *flags:
    cargo build --quiet --package ctrlassist-test
    target/debug/ctrlassist-fixture forward {{ node }} {{ flags }}
