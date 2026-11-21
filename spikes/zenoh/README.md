# Spike: zenoh beside the input loop

Throwaway code for the plan's last spike. It answers one question: does a zenoh session in the same process disturb the input thread?

`src/` builds `zspike`:

- `mux` is the loop spike's input loop on its own thread, inside a process that also runs a zenoh router on a Unix socket with a status publisher, an rpc, a liveliness token and a mirror of what the loop forwards. It takes the loop spike's arguments, so that spike's bench times it unchanged.
- `ping` makes one rpc and exits, as a CLI verb would.
- `watch` subscribes to the status, or with `--mirror` to the mirror.

`just spike-zenoh-bench` runs the comparison. The results and verdict are in `docs/plans/findings/zenoh.md`.
