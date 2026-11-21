# Spike: the input loop

Throwaway code for the plan's input loop spike. It answers: what does the data path look like, what reads the pads, and how does it compare with the legacy 0.4.0 binary?

`src/` builds `loopspike`:

- `mux` forwards two pads to one virtual pad from a single thread blocked in epoll, answering the virtual pad's feedback requests on the same thread.
- `bench` makes two fake pads through uinput, starts a target on them (`mux`, or the legacy binary), and reports latency, CPU and idle wakeups.
- `burn` spins threads, for measuring under load.

`just spike-loop-bench` runs the matrix. The legacy binary is built from a `git worktree` of `legacy/0.4`.

The results are in `docs/architecture.md`, under "What the platform does". This directory is deleted when the device crate replaces it.
