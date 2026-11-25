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

# Its config is `.config/actionlint.yaml`; actionlint only looks under
# .github/ by itself, so the path is passed.
#
# Lint the GitHub workflows: syntax, expressions, runner labels, and their `run:` lines
[group('check')]
actionlint:
    actionlint -config-file .config/actionlint.yaml

# Without a GitHub token in GH_TOKEN it runs offline and says so.
#
# Audit the workflows and the Dependabot config for security problems
[group('check')]
zizmor:
    zizmor .github/

# Run every local gate
[group('check')]
check: deny shear typos actionlint zizmor licenses-html fmt build clippy doc test

# --- CI ------------------------------------------------------------------
#
# What the workflows run, so that a CI failure reproduces locally.

# The line coverage the workspace must keep. Raise it with `just cov` when
# coverage rises; never lower it.
cov_baseline_lines := "95"

# Kept out of the coverage figure: test infrastructure; the hiding
# service's own files, which need its user, its udev rule and real device
# nodes, and are exercised by the virtual machine checks, its client and
# wire format being counted; and the two files of the tray that need a desktop's
# session bus, which hold only what puts the rest on a desktop.
cov_exclude := "--ignore-filename-regex 'ctrlassist-test/|ctrlassist-hider/src/(main|activation|seat|pad|hide)\\.rs|ctrlassist-tray/src/(desktop|service)\\.rs'"

# Advisories and yanked crates, which change without the lockfile changing.
#
# Check the dependencies against the RustSec advisory database
[group('ci')]
audit:
    cargo deny --config packaging/licensing/deny.toml check advisories

# Where `_cov-collect` records the test run's exit status, so the report
# recipes can still render coverage after a failure and then exit with it.
cov_status := "target/llvm-cov-target/cov-test-status"

# Fail with the recorded test status, if the run had failures.
cov_verdict := 'status=$(cat ' + cov_status + ' 2>/dev/null || echo 1); if [ "$status" != 0 ]; then echo "tests failed (exit $status) - coverage reflects a failing run" >&2; exit "$status"; fi'

# Run the tests under coverage instrumentation, for the cov-* recipes.
# Every test runs (`--no-fail-fast`) and a failure is recorded rather
# than aborting, so the report recipes still render what ran; they exit
# with the recorded status afterwards. The gate (`test-ci`) stays
# fail-fast. TisVlay's recipes.
[private]
_cov-collect:
    #!/usr/bin/env bash
    set -uo pipefail
    export CARGO_PROFILE_DEV_DEBUG=line-tables-only
    status=0
    cargo llvm-cov --no-report nextest --workspace --all-features --no-fail-fast || status=$?
    cargo test --workspace --all-features --doc || { rc=$?; [ "$status" = 0 ] && status=$rc; }
    mkdir -p "$(dirname {{ cov_status }})"
    echo "$status" > {{ cov_status }}

# Print a coverage summary
[group('coverage')]
cov: _cov-collect
    cargo llvm-cov report {{ cov_exclude }} --summary-only
    @{{ cov_verdict }}

# Write an HTML coverage report and open it
[group('coverage')]
cov-html: _cov-collect
    cargo llvm-cov report {{ cov_exclude }} --html --open
    @{{ cov_verdict }}

# Write lcov.info for an editor's coverage gutters
[group('coverage')]
cov-lcov: _cov-collect
    cargo llvm-cov report {{ cov_exclude }} --lcov --output-path lcov.info
    @echo "Wrote lcov.info."
    @{{ cov_verdict }}

# A failing test run fails the ratchet before any comparison: partial
# coverage must neither pass nor move the baseline.
#
# Fail if line coverage is below `lines` percent (default: the floor)
[group('coverage')]
cov-ratchet lines=cov_baseline_lines: _cov-collect
    @{{ cov_verdict }}
    cargo llvm-cov report {{ cov_exclude }} --fail-under-lines {{ lines }} --summary-only

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
    export NEXTEST_PROFILE=ci
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
_gui_log := "info,ctrlassist_gui=debug,slint=warn,winit=warn"
gui_mcp := env("GUI_MCP", "true")
gui_live := env("GUI_LIVE", "true")

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

# GUI_MCP and GUI_LIVE (both on unless set to "false") build with
# Slint's MCP server, on http://127.0.0.1:9315/mcp, and with live
# preview, which reloads ui/*.slint from disk on every save without a
# rebuild. Both are features of the slint dependency passed on the
# command line, so a plain build never carries them. The test profile
# keeps the element ids the MCP server needs (ctrlassist-gui's build.rs).
#
# Start the settings window against the dev daemon
[group('dev')]
gui-start:
    #!/usr/bin/env bash
    set -euo pipefail
    env_prefix=(RUST_LOG="{{ _gui_log }}")
    features=()
    if [ "{{ gui_mcp }}" = "true" ]; then
        env_prefix+=(SLINT_MCP_PORT=9315)
        features+=("slint/mcp")
    fi
    if [ "{{ gui_live }}" = "true" ]; then
        env_prefix+=(SLINT_LIVE_PREVIEW=1)
        features+=("slint/live-preview")
    fi
    feature_arg=()
    if [ "${#features[@]}" -gt 0 ]; then
        feature_arg=(--features "$(IFS=,; echo "${features[*]}")")
    fi
    # `env`, since bash doesn't take assignments out of an array
    # expansion as environment assignments.
    env "${env_prefix[@]}" \
        cargo run -p ctrlassist-gui-app "${feature_arg[@]}" --profile {{ _dev_profile }} --bin ctrlassist-gui -- \
            --socket "{{ socket }}"

# The hiding service from `just hider-dev` runs as its own user and is
# left alone: Ctrl-C in its own terminal stops it.
#
# Kill the dev daemon, tray and settings window
[group('dev')]
stop-all:
    # Matched on the dev build's path, so an installed daemon keeps running.
    -pkill -f 'target/(debug|release)/ctrlassist service (daemon|tray)'
    -pkill -f 'target/(debug|release)/ctrlassist-gui'
    # SIGTERM returns at once; wait until the processes are gone so a
    # start that follows can't race the exit. The daemon stops its setups
    # and restores their pads on the way out. Five seconds at most, so a
    # stuck process can't wedge the recipe.
    @timeout 5 sh -c 'while pgrep -f "target/(debug|release)/(ctrlassist service (daemon|tray)|ctrlassist-gui)" > /dev/null; do sleep 0.1; done' || true

# Run the CLI against the dev daemon, such as `just cli status`
[group('dev')]
cli *args:
    cargo run --quiet --profile {{ _dev_profile }} --bin ctrlassist -- --socket "{{ socket }}" {{ args }}

# It also gates: `--fail` stops on any crate whose license `about.toml`
# doesn't accept, so a dependency that brings a new license is noticed
# when it arrives, not when a package is built.
#
# Write the third-party license bundle the packages ship
[group('check')]
licenses-html:
    cargo about generate \
        --config packaging/licensing/about.toml \
        --workspace --all-features --locked --fail \
        packaging/licensing/about.html.hbs \
        -o packaging/licensing/THIRD-PARTY-LICENSES.html

# --- Docs ----------------------------------------------------------------
#
# The user guide is an mdBook under docs/book. The pages under
# src/generated are written from the code they document.

generated_docs := "docs/book/src/generated"

# Build the user guide into docs/book/book/
[group('docs')]
docs-build:
    mdbook build docs/book

# External links are skipped, so the check needs no network and gives the
# same answer each time.
#
# Check local links and heading anchors in the built guide
[group('docs')]
docs-links: docs-build
    lychee --offline --include-fragments=anchor-only \
        --root-dir "$PWD/docs/book/book" docs/book/book/

# Serve the user guide with live reload at http://localhost:3000
[group('docs')]
docs-serve:
    mdbook serve docs/book --open

# Rewrite the guide's generated pages from the code they document
[group('docs')]
docs-gen:
    cargo run -q -p ctrlassist-docgen -- {{ generated_docs }}
    @echo "Wrote {{ generated_docs }}/: review and commit."

# Fail if a generated page is stale or a new one isn't committed
[group('docs')]
docs-gen-check: docs-gen
    git diff --exit-code -- {{ generated_docs }}
    test -z "$(git ls-files --others --exclude-standard -- {{ generated_docs }})"

# --- Packaging -----------------------------------------------------------

# The oldest glibc the packaged binaries run on. 2.34 covers Debian 12,
# Ubuntu 22.04, Fedora 35 and everything rolling. TisVlay's baseline.
glibc_baseline := "2.34"

# Architecture the packages are for: the host's by default. ARCH=aarch64
# on an x86_64 host cross-builds, which needs that Rust target installed.
package_arch := env("ARCH", arch())
_package_triple := package_arch + "-unknown-linux-gnu"

# Built with the `dist` profile, by zig as the linker, so the binaries
# name the standard ELF interpreter and no glibc symbol newer than the
# baseline. Which crates are built comes from `default-members` in the
# root Cargo.toml. cargo-auditable embeds each binary's dependency list.
[private]
package-build:
    cargo auditable zigbuild --profile dist \
        --target {{ _package_triple }}.{{ glibc_baseline }}
    ln -sfn {{ _package_triple }}/dist target/nfpm-dist

# Built for the host on purpose: the packaged binary targets another
# glibc and may not run here, while what it prints for registration names
# only the shell and the command.
#
# Generate what registers the command's completions with each shell
[group('package')]
completions:
    cargo build --package ctrlassist-cli
    mkdir -p packaging/completions
    COMPLETE=bash target/debug/ctrlassist > packaging/completions/ctrlassist.bash
    COMPLETE=zsh  target/debug/ctrlassist > packaging/completions/ctrlassist.zsh
    COMPLETE=fish target/debug/ctrlassist > packaging/completions/ctrlassist.fish

# Build the .deb
[group('package')]
package-deb: licenses-html completions package-build
    cargo nfpm package -p ctrlassist-cli --no-build --profile dist --target {{ _package_triple }} -f deb

# Build the .rpm
[group('package')]
package-rpm: licenses-html completions package-build
    cargo nfpm package -p ctrlassist-cli --no-build --profile dist --target {{ _package_triple }} -f rpm

# Build the Arch package
[group('package')]
package-archlinux: licenses-html completions package-build
    cargo nfpm package -p ctrlassist-cli --no-build --profile dist --target {{ _package_triple }} -f archlinux

# Build the .deb, the .rpm and the Arch package
[group('package')]
package-all: package-deb package-rpm package-archlinux

# What each container is asked after installing its package: the command
# runs on that distribution's glibc and finds its libraries, says there is
# no daemon, and completes its own words; the hiding service runs and
# refuses what isn't a pad; the window's binary runs as far as saying its
# release, which needs fontconfig, which it links, and not the libraries
# it loads at run time to draw with; and everything the package ships is
# in place.
_package_checks := '''
    set -eu
    ctrlassist --version
    ctrlassist status --json 2>/dev/null | grep -q '"alive":false'
    COMPLETE=fish ctrlassist -- ctrlassist pro | grep -q profile
    ctrlassist-hider check /dev/null | grep -q refused
    ctrlassist-gui --version
    for file in \
        /usr/share/applications/io.github.ctrlassist.Gui.desktop \
        /usr/share/dbus-1/services/io.github.ctrlassist.Gui.service \
        /usr/lib/systemd/system/ctrlassist-hider.socket \
        /usr/lib/systemd/system/ctrlassist-hider.service \
        /usr/lib/systemd/user/ctrlassist-daemon.service \
        /usr/lib/systemd/user/ctrlassist-tray.service \
        /usr/lib/systemd/system/user@.service.d/10-ctrlassist.conf \
        /usr/lib/udev/rules.d/70-ctrlassist.rules \
        /usr/lib/udev/rules.d/72-ctrlassist-hide.rules \
        /usr/lib/sysusers.d/ctrlassist.conf \
        /usr/lib/modules-load.d/ctrlassist.conf \
        /usr/share/bash-completion/completions/ctrlassist \
        /usr/share/zsh/site-functions/_ctrlassist \
        /usr/share/fish/vendor_completions.d/ctrlassist.fish \
        /usr/share/doc/ctrlassist/NOTICE \
        /usr/share/doc/ctrlassist/THIRD-PARTY-LICENSES.html
    do test -s "$file" || { echo "missing: $file"; exit 1; }; done
    echo "installed and working"
'''

# Needs docker. Each package is installed by its distribution's own
# package manager, which also resolves the libraries it declares, in a
# container that has never seen the build. Ubuntu 22.04 is the oldest
# glibc the baseline covers. The containers have no systemd, so the units
# are checked for being there, not for starting: the NixOS VM check starts
# them.
#
# Install each package in a clean container of its distribution and try it
[group('package')]
package-test: package-all
    #!/usr/bin/env bash
    set -euo pipefail
    dist=target/{{ _package_triple }}/dist
    try() {
        local image=$1 install=$2
        echo "--- $image"
        docker run --rm -v "$PWD/$dist:/dist:ro" "$image" sh -c "$install && $checks"
    }
    checks={{ quote(_package_checks) }}
    try debian:12 'apt-get update -qq && apt-get install -y -qq /dist/ctrlassist_*.deb >/dev/null'
    # Ubuntu's image tells dpkg to leave out /usr/share/doc; a real
    # install doesn't, so that is lifted before installing.
    try ubuntu:22.04 'rm -f /etc/dpkg/dpkg.cfg.d/excludes && apt-get update -qq && apt-get install -y -qq /dist/ctrlassist_*.deb >/dev/null'
    try fedora:latest 'dnf install -y -q /dist/ctrlassist-*.rpm >/dev/null'
    # Arch's image does the same through pacman's NoExtract, and comes
    # with no package database, which the window's libraries are
    # resolved from.
    try archlinux:latest "sed -i '/^NoExtract/d' /etc/pacman.conf && pacman -Sy --noconfirm >/dev/null && pacman -U --noconfirm /dist/ctrlassist-*.pkg.tar.zst >/dev/null"

sbom_dir := "target/sbom"

# CycloneDX 1.5 JSON, with licenses and checksums, written to target/sbom/
# as <binary>-<version>-<arch>.cdx.json; the release attaches them. Each
# is limited to its binary's target and default features, as the release
# build is. cargo-cyclonedx works from cargo metadata, not the build, so
# `just sbom-check` compares them with the built binaries. It writes one
# file per workspace binary next to each crate's Cargo.toml, so the recipe
# keeps the shipped three and deletes the rest. SOURCE_DATE_EPOCH (the
# commit time) makes reruns identical, but it also makes cargo-cyclonedx
# leave out the serial number, which actions/attest requires of a
# CycloneDX SBOM. So each gets a name-based UUID (v5) for its commit,
# binary and architecture: the same build reruns to the same serial, and
# any other build gets a new one. TisVlay's recipe.
#
# Write one SBOM per shipped binary
[group('package')]
sbom:
    #!/usr/bin/env bash
    set -euo pipefail
    rm -rf {{ sbom_dir }} && mkdir -p {{ sbom_dir }}
    SOURCE_DATE_EPOCH=$(git log -1 --format=%ct) cargo cyclonedx --quiet \
        --describe binaries --target {{ _package_triple }} \
        --format json --spec-version 1.5
    commit=$(git rev-parse HEAD)
    for shipped in ctrlassist-cli/ctrlassist ctrlassist-hider/ctrlassist-hider ctrlassist-gui-app/ctrlassist-gui; do
        crate=${shipped%/*} bin=${shipped#*/}
        generated=crates/$crate/${bin}_bin.cdx.json
        version=$(jq -r .metadata.component.version "$generated")
        serial=$(uuidgen --sha1 --namespace @url \
            --name "https://github.com/CtrlAssist/CtrlAssist/commit/$commit#$bin-{{ package_arch }}")
        jq --arg serial "urn:uuid:$serial" '.serialNumber = $serial' "$generated" \
            > "{{ sbom_dir }}/$bin-$version-{{ package_arch }}.cdx.json"
    done
    rm -f crates/*/*_bin.cdx.json
    ls {{ sbom_dir }}

# Run after `just package-all`. The root crate is the one expected
# difference: the SBOM describes it as the binary itself, in
# metadata.component, so it's left out of the comparison.
#
# Fail if an SBOM lists different crates than its release binary embeds
[group('package')]
sbom-check: sbom
    #!/usr/bin/env bash
    set -euo pipefail
    status=0
    for sbom in {{ sbom_dir }}/*.cdx.json; do
        bin=$(jq -r .metadata.component.name "$sbom")
        binary=target/nfpm-dist/$bin
        [ -f "$binary" ] || { echo "$binary is missing: run \`just package-all\` first" >&2; exit 1; }
        listed=$(jq -r '.components[] | "\(.name)@\(.version)"' "$sbom" | sort -u)
        embedded=$(rust-audit-info "$binary" | jq -r '.packages[] | select(.root != true) | "\(.name)@\(.version)"' | sort -u)
        if diff -u --label "$sbom" --label "$binary (embedded)" <(echo "$listed") <(echo "$embedded"); then
            echo "$bin: $(wc -l <<< "$listed") crates, SBOM and binary agree"
        else
            status=1
        fi
    done
    exit "$status"

# For the tag ref that built them (refs/tags/vX.Y.Z), it checks:
#   - the checksums, when checksums-sha256.txt is there;
#   - provenance for every package and for the binaries unpacked from them
#     (what an installed system has), and each binary's SBOM attestation,
#     both looked up on GitHub and checked from the release's Sigstore
#     bundles alone;
#   - that each SBOM asset is the SBOM that was signed.
# release.yml runs it before it creates the draft, and `just
# verify-release` runs it on the draft. The signer path is also in
# build-packages.yml's header; keep the two in step. TisVlay's recipe.
#
# Verify a release's assets in <dir> the way a user would
[group('package')]
verify-assets dir ref:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s failglob
    signer=CtrlAssist/CtrlAssist/.github/workflows/build-packages.yml
    identity=(--repo CtrlAssist/CtrlAssist --signer-workflow "$signer" --source-ref "{{ ref }}" --deny-self-hosted-runners)
    sbom_type=https://cyclonedx.org/bom
    cd "{{ dir }}"
    if [ -f checksums-sha256.txt ]; then
        sha256sum --quiet -c checksums-sha256.txt
        echo "checksums-sha256.txt: OK"
    fi
    version=$(jq -r .metadata.component.version ctrlassist-hider-*-x86_64.cdx.json)
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    # An architecture's three packages must carry identical binaries: unpack
    # them all, and keep one copy per architecture to verify.
    for pkg in ctrlassist_*.deb ctrlassist-*.rpm ctrlassist-*.pkg.tar.zst; do
        case $pkg in
            *_amd64.deb | *.x86_64.rpm | *-x86_64.pkg.tar.zst) arch=x86_64 ;;
            *_arm64.deb | *.aarch64.rpm | *-aarch64.pkg.tar.zst) arch=aarch64 ;;
            *) echo "$pkg: unknown architecture" >&2; exit 1 ;;
        esac
        root=$work/unpacked/$pkg
        mkdir -p "$root" "$work/$arch"
        # rpm members are absolute; bsdtar strips the leading `/` and says so.
        case $pkg in
            *.deb) bsdtar -xOf "$pkg" 'data.tar*' | bsdtar -xf - -C "$root" ;;
            *) if ! err=$(bsdtar -xf "$pkg" -C "$root" 2>&1); then echo "$err" >&2; exit 1; fi ;;
        esac
        for bin in ctrlassist ctrlassist-hider ctrlassist-gui; do
            if [ ! -f "$work/$arch/$bin" ]; then
                cp "$root/usr/bin/$bin" "$work/$arch/$bin"
            elif ! cmp -s "$root/usr/bin/$bin" "$work/$arch/$bin"; then
                echo "$pkg: its $bin differs from the other $arch packages'" >&2
                exit 1
            fi
        done
    done
    for arch in x86_64 aarch64; do
        deb_arch=$([ "$arch" = x86_64 ] && echo amd64 || echo arm64)
        packages=(ctrlassist_*_"$deb_arch".deb ctrlassist-*."$arch".rpm ctrlassist-*-"$arch".pkg.tar.zst)
        provenance=ctrlassist-$version-$arch.provenance.sigstore.json
        for subject in "${packages[@]}" "$work/$arch/ctrlassist" "$work/$arch/ctrlassist-hider" "$work/$arch/ctrlassist-gui"; do
            gh attestation verify "$subject" "${identity[@]}" > /dev/null
            gh attestation verify "$subject" "${identity[@]}" --bundle "$provenance" > /dev/null
            echo "${subject#"$work"/}: provenance OK"
        done
        for bin in ctrlassist ctrlassist-hider ctrlassist-gui; do
            sbom=$bin-$version-$arch.cdx.json
            bundle=$bin-$version-$arch.cdx.sigstore.json
            for subject in "$work/$arch/$bin" "${packages[@]}"; do
                gh attestation verify "$subject" "${identity[@]}" --predicate-type "$sbom_type" > /dev/null
                gh attestation verify "$subject" "${identity[@]}" --predicate-type "$sbom_type" --bundle "$bundle" > /dev/null
            done
            signed=$(gh attestation verify "$work/$arch/$bin" "${identity[@]}" --predicate-type "$sbom_type" \
                --bundle "$bundle" --format json --jq '.[0].verificationResult.statement.predicate')
            if ! diff -q <(jq -S . <<< "$signed") <(jq -S . "$sbom") > /dev/null; then
                echo "$sbom differs from the SBOM signed in $bundle" >&2
                exit 1
            fi
            echo "$arch/$bin: SBOM OK, for it and its packages; $sbom is the signed one"
        done
    done

# It downloads into target/verify/<tag>/ (a draft too, with your gh
# login), runs `just verify-assets` on that, and runs the cosign check
# that needs no GitHub account. Once the release is published it also
# checks GitHub's release attestation, which immutable releases only
# create on publishing.
#
# Download a release's assets and verify them
[group('package')]
verify-release tag:
    #!/usr/bin/env bash
    set -euo pipefail
    repo=CtrlAssist/CtrlAssist
    dir=target/verify/{{ tag }}
    rm -rf "$dir" && mkdir -p "$dir"
    gh release download "{{ tag }}" --repo "$repo" --dir "$dir"
    just verify-assets "$dir" "refs/tags/{{ tag }}"
    version=$(jq -r .metadata.component.version "$dir"/ctrlassist-hider-*-x86_64.cdx.json)
    cosign verify-blob-attestation \
        --bundle "$dir/ctrlassist-$version-x86_64.provenance.sigstore.json" \
        --certificate-identity "https://github.com/$repo/.github/workflows/build-packages.yml@refs/tags/{{ tag }}" \
        --certificate-oidc-issuer https://token.actions.githubusercontent.com \
        --type https://slsa.dev/provenance/v1 \
        "$dir/ctrlassist_$version-1_amd64.deb"
    if [ "$(gh release view "{{ tag }}" --repo "$repo" --json isDraft --jq .isDraft)" = true ]; then
        echo "{{ tag }} is a draft: rerun once it's published, to check GitHub's release attestation too."
    else
        gh release verify "{{ tag }}" --repo "$repo"
        for asset in "$dir"/*; do
            gh release verify-asset "{{ tag }}" "$asset" --repo "$repo" > /dev/null
        done
        echo "release attestation: OK for every asset"
    fi

# release-plz picks the version from the commit history and writes the
# CHANGELOGs; the guide's generated pages follow, the release version
# among them.
#
# Bump the version and CHANGELOGs for a release
[group('package')]
release-bump: && docs-gen
    release-plz update --config .config/release-plz.toml

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

hider_dev_dir := "/run/ctrlassist-dev"
hider_dev_socket := hider_dev_dir / "hider.sock"

# The service runs as its own user, ctrlassist-hider, as installed, and
# hides pads through the installed udev rule, which names its marker
# directory: so the machine needs this tree's rule and user installed (on
# NixOS, the module from this tree; docs/testing.md says how to check).
# The installed service shares that directory and empties it as it starts
# and as it stops, so the recipe refuses while its socket or service is
# active, and says how to stop it. It doesn't stop it itself: that would
# show again whatever the installed service has hidden for a running
# daemon, and starting it again would rest on the recipe ending cleanly,
# where a closed terminal would leave the machine without its service and
# nothing to say so. The binary is copied out of the build tree, which the
# service's user may not be able to read. Asks for sudo. Leave it running
# and use `just hide-pad` in another terminal; Ctrl-C stops it, and
# whatever it hid is then shown again, as the unit's ExecStopPost does.
#
# Run the hiding service from the build tree, as its own user
[group('hardware')]
hider-dev:
    #!/usr/bin/env bash
    set -euo pipefail
    rule=72-ctrlassist-hide.rules
    if ! getent passwd ctrlassist-hider >/dev/null || ! getent group ctrlassist-hider >/dev/null \
        || ! [[ -e /etc/udev/rules.d/$rule || -e /usr/lib/udev/rules.d/$rule ]]; then
        echo "The ctrlassist-hider user and group and the udev rule $rule must be installed" >&2
        echo "from this tree first: its package, or on NixOS its module. docs/testing.md has how." >&2
        exit 1
    fi
    if systemctl is-active --quiet ctrlassist-hider.socket ctrlassist-hider.service; then
        echo "The installed hiding service is active. It shares /run/ctrlassist/hide with this one" >&2
        echo "and empties it as it starts and as it stops. Stop it first, which shows again" >&2
        echo "whatever it has hidden, and start its socket again when done:" >&2
        echo "  sudo systemctl stop ctrlassist-hider.socket ctrlassist-hider.service" >&2
        echo "  sudo systemctl start ctrlassist-hider.socket" >&2
        exit 1
    fi
    cargo build --quiet --package ctrlassist-hider
    sudo install -d -o ctrlassist-hider -g ctrlassist-hider -m 0755 {{ hider_dev_dir }}
    sudo install -d -o ctrlassist-hider -g ctrlassist-hider -m 0700 /run/ctrlassist/hide
    sudo install -o root -g root -m 0755 target/debug/ctrlassist-hider {{ hider_dev_dir }}/ctrlassist-hider
    as_service=(sudo -u ctrlassist-hider -g ctrlassist-hider {{ hider_dev_dir }}/ctrlassist-hider)
    # Ctrl-C ends the service, not this script, which then reconciles.
    trap : INT
    "${as_service[@]}" --listen {{ hider_dev_socket }} || true
    "${as_service[@]}" reconcile

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
# events a second each. Compare with the table in docs/architecture.md,
# whose numbers were taken with `--release`; the bench builds with the
# `dist` profile, as the packages are, so that it times what ships.
#
# Time the input path, idle and with every CPU busy
[group('hardware')]
bench seconds="10":
    cargo build --quiet --profile dist --package ctrlassist-test
    target/dist/ctrlassist-bench --seconds {{ seconds }}
    target/dist/ctrlassist-bench --seconds {{ seconds }} --load "$(nproc)"

# The same with two fake DualSenses read for motion as well, each report
# a new reading, into a virtual DualSense, at the 250 reports a second a
# DualSense sends over USB. It opens the fake pads' raw nodes, which a
# seat's user is given where a PlayStation pad's is.
#
# Time the input path with motion, idle and with every CPU busy
[group('hardware')]
bench-motion seconds="10":
    cargo build --quiet --profile dist --package ctrlassist-test
    target/dist/ctrlassist-bench --seconds {{ seconds }} --rate 250 --motion
    target/dist/ctrlassist-bench --seconds {{ seconds }} --rate 250 --motion --load "$(nproc)"

# The same measurement with the input thread inside a daemon, which the
# bench runs as a child with a profile over its two fake pads: what the
# control plane beside the input thread costs it.
#
# Time the input path through the daemon, idle and with every CPU busy
[group('hardware')]
bench-daemon seconds="10":
    cargo build --quiet --profile dist --package ctrlassist-test --package ctrlassist-cli
    target/dist/ctrlassist-bench --seconds {{ seconds }} --daemon target/dist/ctrlassist
    target/dist/ctrlassist-bench --seconds {{ seconds }} --daemon target/dist/ctrlassist --load "$(nproc)"

# Pass the pad's event node, and `--xbox` for the Xbox identity. Needs
# uinput, which the packaged udev rule or Steam's own gives the seat user.
# Ctrl-C removes the virtual pad.
#
# Put a virtual pad in front of a real pad
[group('hardware')]
forward-pad node *flags:
    cargo build --quiet --package ctrlassist-test
    target/debug/ctrlassist-fixture forward {{ node }} {{ flags }}
