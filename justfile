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

# Starts Steam if it isn't running, opens the pad's hidraw node as Steam
# would, hides the pad with the udev rule, re-binds its driver, then
# restores, logging at each stage what the already-open reader and Steam
# still see. The pad must be on USB. Asks for the sudo password.
#
# Does hiding dislodge a program that already has the pad open?
[group('spike')]
spike-hiding-dislodge vendor="054c" product="0ce6":
    #!/usr/bin/env bash
    set -euo pipefail
    probe=spikes/hiding/target/debug/probe
    log=spikes/hiding/target/dislodge.log
    rule=/run/udev/rules.d/72-ctrlassist-spike.rules
    steam_log=$HOME/.local/share/Steam/logs/controller.txt
    (cd spikes/hiding && cargo build --quiet)
    : > "$log"
    say() { echo "===== $1 =====" | tee -a "$log"; }
    # The pad's HID device and hidraw node, from its vendor and product.
    hid=$("$probe" pads | awk -v id="vendor={{ vendor }} product={{ product }}" \
        'index($0, id) { found = 1 } found && $1 == "hid" { print $2; exit }')
    hidraw=$("$probe" pads | awk -v hid="$hid" \
        '$1 == "hid" { found = ($2 == hid) } found && $1 == "node" && $2 == "hidraw" { print $3; exit }')
    driver=$(basename "$(readlink "/sys/bus/hid/devices/$hid/driver")")
    echo "pad: hid=$hid hidraw=$hidraw driver=$driver" | tee -a "$log"
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
            | grep -E "Local Device Found|type:|Product:|device closed|device opened|Hiding" || true
        steam_mark=$(wc -l < "$steam_log")
    }
    rebind() {
        echo "$hid" | sudo tee "/sys/bus/hid/drivers/$driver/unbind" > /dev/null || true
        echo "$hid" | sudo tee "/sys/bus/hid/drivers/$driver/bind" > /dev/null
        sudo udevadm settle
    }
    restore() {
        sudo rm -f "$rule"
        sudo udevadm control --reload
        rebind
    }
    trap restore EXIT
    # A reader that holds the node open across everything, as Steam does.
    "$probe" listen "$hidraw" --millis 40000 --progress >> "$log" 2>&1 &
    reader=$!
    say "before: reader running, Steam's view"
    sleep 4
    steam_since | tee -a "$log"
    say "rule installed (nodes denied); is the open reader still receiving?"
    sudo mkdir -p /run/udev/rules.d
    printf '%s\n' \
        'SUBSYSTEM=="input", ATTRS{id/vendor}=="{{ vendor }}", ATTRS{id/product}=="{{ product }}", MODE="0000", GROUP="root", TAG-="uaccess"' \
        'SUBSYSTEM=="hidraw", ATTRS{idVendor}=="{{ vendor }}", ATTRS{idProduct}=="{{ product }}", MODE="0000", GROUP="root", TAG-="uaccess"' \
        | sudo tee "$rule" > /dev/null
    sudo udevadm control --reload
    sudo udevadm trigger --action=add --subsystem-match=input --subsystem-match=hidraw
    sudo udevadm settle
    sleep 6
    steam_since | tee -a "$log"
    say "driver re-bound with the rule in place; reader and Steam now?"
    rebind
    sleep 6
    steam_since | tee -a "$log"
    "$probe" access 2>&1 | tee -a "$log"
    say "rule removed and driver re-bound; does Steam find the pad again?"
    restore
    trap - EXIT
    sleep 8
    steam_since | tee -a "$log"
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

# Run the routing graph spike's tests and its step benchmark
[group('spike')]
spike-graph:
    cd spikes/graph && cargo test && cargo run --release --example bench

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
