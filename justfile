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

# Format all Rust
[group('check')]
fmt:
    cargo fmt

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

# Run every local gate
[group('check')]
check: typos fmt build clippy doc test

# --- Spikes --------------------------------------------------------------

# The observations are in the build log:
# `nix log .#checks.x86_64-linux.spike-hiding`.
#
# Run the hiding spike's clean-machine probes in a NixOS VM
[group('spike')]
spike-hiding-vm:
    nix build .#checks.x86_64-linux.spike-hiding --print-build-logs

# Hides the pad with a runtime udev rule, records what the seat user and SDL
# still see, asks for a replug, then removes the rule and puts the nodes'
# modes back. vendor/product are the pad's USB IDs in hex.
#
# Root half of the hiding spike on a real pad (asks for the sudo password)
[group('spike')]
spike-hiding-root vendor="054c" product="0ce6":
    #!/usr/bin/env bash
    set -euo pipefail
    probe=spikes/hiding/target/debug/probe
    log=spikes/hiding/target/root-udev.log
    rule=/run/udev/rules.d/72-ctrlassist-spike.rules
    (cd spikes/hiding && cargo build --quiet)
    observe() {
        echo "===== $1 =====" | tee -a "$log"
        { "$probe" access; echo "-- SDL, HIDAPI on:"; sdl2-jstest --list | grep -E "^Found|Joystick Name" || true
          echo "-- SDL, HIDAPI off:"; SDL_JOYSTICK_HIDAPI=0 sdl2-jstest --list | grep -E "^Found|Joystick Name" || true; } 2>&1 | tee -a "$log"
    }
    # Removing the rule doesn't reset a node's mode or group, so put back
    # what was recorded before hiding.
    saved=$("$probe" pads | awk '$1 == "node" { print $3 }' | xargs -r stat -c '%a %U:%G %n')
    restore() {
        sudo rm -f "$rule"
        sudo udevadm control --reload
        sudo udevadm trigger --action=add --subsystem-match=input --subsystem-match=hidraw
        sudo udevadm settle
        while read -r mode owner node; do
            [ -e "$node" ] && sudo chmod "$mode" "$node" && sudo chown "$owner" "$node"
        done <<< "$saved"
        sudo udevadm trigger --action=change --subsystem-match=input --subsystem-match=hidraw
        sudo udevadm settle
    }
    : > "$log"
    observe "before"
    trap restore EXIT
    sudo mkdir -p /run/udev/rules.d
    printf '%s\n' \
        'SUBSYSTEM=="input", ATTRS{id/vendor}=="{{ vendor }}", ATTRS{id/product}=="{{ product }}", MODE="0000", GROUP="root", TAG-="uaccess"' \
        'SUBSYSTEM=="hidraw", ATTRS{idVendor}=="{{ vendor }}", ATTRS{idProduct}=="{{ product }}", MODE="0000", GROUP="root", TAG-="uaccess"' \
        | sudo tee "$rule" > /dev/null
    sudo udevadm control --reload
    sudo udevadm trigger --action=add --subsystem-match=input --subsystem-match=hidraw
    sudo udevadm settle
    observe "hidden by the udev rule"
    read -r -p "Unplug the pad, plug it back in, then press Enter: "
    sudo udevadm settle
    observe "hidden, after a reconnect"
    restore
    trap - EXIT
    observe "after the rule is removed"
    echo "Wrote $log"

# Starts Steam if it isn't running, holds the pad open as a game would,
# hides the pad with the udev rule, re-binds its driver, then restores,
# logging at each stage what the already-open reader and Steam still see.
# Works for a HID pad on USB or Bluetooth (re-binds its HID driver) and for
# a pad with no HID device, such as an Xbox pad on USB (re-binds its USB
# interface). Pass the pad's vendor and product in hex. Asks for sudo.
#
# Does hiding dislodge a program that already has the pad open?
[group('spike')]
spike-hiding-dislodge vendor="054c" product="0ce6":
    #!/usr/bin/env bash
    set -euo pipefail
    probe=spikes/hiding/target/debug/probe
    log=spikes/hiding/target/dislodge-{{ vendor }}-{{ product }}.log
    rule=/run/udev/rules.d/72-ctrlassist-spike.rules
    steam_log=$HOME/.local/share/Steam/logs/controller.txt
    (cd spikes/hiding && cargo build --quiet)
    : > "$log"
    say() { echo "===== $1 =====" | tee -a "$log"; }
    # One pad's block of `probe pads`, picked by its vendor and product.
    block=$("$probe" pads | awk -v id="vendor={{ vendor }} product={{ product }}" \
        '/^pad / { if (found) exit; text = "" } { text = text $0 "\n" } index($0, id) { found = 1 } END { if (found) printf "%s", text }')
    if [ -z "$block" ]; then
        echo "no connected pad with vendor {{ vendor }} product {{ product }}" >&2
        exit 1
    fi
    field() { echo "$block" | awk -v key="$1" '$1 == key { print $2; exit }'; }
    node() { echo "$block" | awk -v kind="$1" '$1 == "node" && $2 == kind { print $3; exit }'; }
    hid=$(field hid)
    if [ -n "$hid" ]; then
        # A HID pad: re-bind its HID driver, and watch its hidraw node.
        device=$hid
        bus=hid
        watch=$(node hidraw)
        hidraw_rule='SUBSYSTEM=="hidraw", KERNELS=="'$hid'", MODE="0000", GROUP="root", TAG-="uaccess"'
    else
        # No HID device: re-bind the USB interface, and watch the event node.
        interface=$(dirname "$(dirname "$(field syspath)")")
        device=$(basename "$interface")
        bus=usb
        watch=$(node event)
        hidraw_rule=""
    fi
    driver=$(basename "$(readlink "/sys/bus/$bus/devices/$device/driver")")
    echo "pad: bus=$bus device=$device driver=$driver watching=$watch" | tee -a "$log"
    echo "$block" | tee -a "$log"
    started_steam=false
    if ! pgrep -x steam > /dev/null; then
        setsid nohup steam -silent > /dev/null 2>&1 < /dev/null &
        started_steam=true
        echo "started Steam; waiting for it to find the pad" | tee -a "$log"
        sleep 25
    fi
    steam_mark=$(wc -l < "$steam_log")
    steam_since() {
        tail -n "+$((steam_mark + 1))" "$steam_log" \
            | grep -E "Local Device Found|type:|Product:|device closed|device opened|Hiding" >> "$log" || true
        steam_mark=$(wc -l < "$steam_log")
        tail -n 8 "$log"
    }
    rebind() {
        echo "$device" | sudo tee "/sys/bus/$bus/drivers/$driver/unbind" > /dev/null || echo "unbind failed" | tee -a "$log"
        echo "$device" | sudo tee "/sys/bus/$bus/drivers/$driver/bind" > /dev/null || echo "bind failed" | tee -a "$log"
        sudo udevadm settle
    }
    restore() {
        sudo rm -f "$rule"
        sudo udevadm control --reload
        rebind
    }
    trap restore EXIT
    # A reader that holds the node open across everything, as a game does.
    "$probe" listen "$watch" --millis 40000 --progress >> "$log" 2>&1 &
    reader=$!
    say "before: reader running, Steam's view"
    sleep 4
    steam_since
    say "rule installed (nodes denied); is the open reader still alive?"
    sudo mkdir -p /run/udev/rules.d
    printf '%s\n' \
        'SUBSYSTEM=="input", ATTRS{id/vendor}=="{{ vendor }}", ATTRS{id/product}=="{{ product }}", MODE="0000", GROUP="root", TAG-="uaccess"' \
        "$hidraw_rule" \
        | sudo tee "$rule" > /dev/null
    sudo udevadm control --reload
    sudo udevadm trigger --action=add --subsystem-match=input --subsystem-match=hidraw
    sudo udevadm settle
    sleep 6
    steam_since
    say "driver re-bound with the rule in place; reader and Steam now?"
    rebind
    sleep 6
    steam_since
    "$probe" pads 2>&1 | grep -c '^pad ' | sed 's/^/pads listed: /' | tee -a "$log"
    "$probe" access 2>&1 | tee -a "$log"
    say "rule removed and driver re-bound; does Steam find the pad again?"
    restore
    trap - EXIT
    sleep 10
    steam_since
    "$probe" access 2>&1 | tee -a "$log"
    kill "$reader" 2> /dev/null || true
    if "$started_steam"; then
        steam -shutdown > /dev/null 2>&1 || true
    fi
    echo "Wrote $log"

# For comparing identifiers across reconnects and transports, e.g.
# `just spike-hiding-ids "dualsense usb"`.
#
# Append the pads' identifiers to a log under a label
[group('spike')]
spike-hiding-ids label:
    (cd spikes/hiding && cargo build --quiet)
    echo "===== {{ label }} =====" >> spikes/hiding/target/ids.log
    spikes/hiding/target/debug/probe pads | tee -a spikes/hiding/target/ids.log

# Needs the legacy binary: `git worktree add ../legacy-0.4 legacy/0.4`, then
# `nix develop -c cargo build --release` inside it.
#
# Time the new loop (with and without real-time scheduling) and the legacy binary, idle and loaded
[group('spike')]
spike-loop-bench legacy="../legacy-0.4/target/release/ctrlassist" runs="3":
    #!/usr/bin/env bash
    set -euo pipefail
    (cd spikes/loop && cargo build --release --quiet)
    bench=spikes/loop/target/release/loopspike
    for load in 0 "$(nproc)"; do
        for _ in $(seq {{ runs }}); do
            "$bench" bench --target ours --load "$load"
            "$bench" bench --target ours --rt --load "$load"
            "$bench" bench --target legacy --legacy-bin {{ legacy }} --load "$load"
        done
    done

# Time the input loop alone, inside a zenoh daemon with an idle client, and with a mirror subscriber
[group('spike')]
spike-zenoh-bench runs="3":
    #!/usr/bin/env bash
    set -euo pipefail
    (cd spikes/loop && cargo build --release --quiet)
    (cd spikes/zenoh && cargo build --release --quiet)
    bench=spikes/loop/target/release/loopspike
    daemon=$PWD/spikes/zenoh/target/release/zspike
    for load in 0 "$(nproc)"; do
        for rt in "" "--rt"; do
            for _ in $(seq {{ runs }}); do
                "$bench" bench --target ours $rt --load "$load"
                "$bench" bench --target ours $rt --load "$load" --exe "$daemon" --with "$daemon watch"
                "$bench" bench --target ours $rt --load "$load" --exe "$daemon" --with "$daemon watch" --with "$daemon watch --mirror"
            done
        done
    done
