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

# typos doesn't search `.config/` yet, so the config path is explicit.
#
# Spell-check code, docs and identifiers, enforcing American English
[group('check')]
typos:
    typos --config .config/typos.toml

# Run every local gate
[group('check')]
check: typos

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

# Run the routing graph spike's tests and its step benchmark
[group('spike')]
spike-graph:
    cd spikes/graph && cargo test && cargo run --release --example bench
