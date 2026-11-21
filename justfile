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

# Kept out of the coverage figure: test infrastructure, and the hiding
# service's own files, which need root and real device nodes. Both are
# exercised by the virtual machine checks. The service's client and wire
# format are counted.
cov_exclude := "--ignore-filename-regex 'ctrlassist-test/|ctrlassist-hider/src/(main|activation|seat|pad|hide)\\.rs'"

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

# Pass the pad's event node, and `--xbox` for the Xbox identity. Needs
# uinput, which the packaged udev rule or Steam's own gives the seat user.
# Ctrl-C removes the virtual pad.
#
# Put a virtual pad in front of a real pad
[group('hardware')]
forward-pad node *flags:
    cargo build --quiet --package ctrlassist-test
    target/debug/ctrlassist-fixture forward {{ node }} {{ flags }}
