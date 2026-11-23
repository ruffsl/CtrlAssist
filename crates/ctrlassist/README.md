# ctrlassist

Client library for CtrlAssist, the Linux daemon that lets several game controllers act as one virtual controller, or one as several. It reaches a CtrlAssist daemon, checks that the daemon's release is one it can talk to, and watches the daemon's state.

This is the library. The `ctrlassist` command is the `ctrlassist-cli` crate's binary.

## Example

```rust,no_run
use ctrlassist::{DaemonClient, StateWatcher, StateWatcherConfig, default_socket_path};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let daemon = DaemonClient::connect(&default_socket_path()).await?;
    // The client mirrors the daemon's published state in the background;
    // wait for that state to arrive (`StateEvent::Hydrated`) before
    // reading it, so an absent value means absent.
    let mut events = StateWatcher::new(&daemon, None, StateWatcherConfig::default());
    while !daemon.is_hydrated() {
        events.next().await;
    }
    if let Some(state) = daemon.daemon_state()? {
        tracing::info!(release = state.version, realtime = state.realtime, "the daemon");
    }
    Ok(())
}
```

## How it works

- **Connecting.** `DaemonClient::connect` returns once its local session is open. It neither waits for nor requires the daemon: the link dials in the background for as long as the client lives, availability follows the daemon's liveliness (`is_available`, and the `DaemonAppeared` and `DaemonDisappeared` events), and `is_hydrated` (the `Hydrated` event) says when the daemon's published state has arrived, so an empty read means empty rather than not yet known.
- **Mirror reads** send nothing. The client keeps a mirror of the state the daemon publishes, so a read answers at once from the last published state. The mirror empties while the daemon is down, with an event for what it drops, and fills again from the daemon's caches when it returns, so nothing it shows outlives the daemon.
- **Rpcs** are round trips. Any rpc can fail with `Error::Unavailable`, `Error::Timeout`, `Error::VersionMismatch`, `Error::Transport` or `Error::Protocol`, each naming the service the call went to. A refusal is `Error::Rpc`, carrying the service's `ErrorCode`, which is `ErrorCode::Unknown` for a code a later release added. A write that waits on work no fixed limit fits (starting, stopping and updating a setup, keeping or dropping a profile, forgetting a pad, a reload) waits for its answer for as long as the daemon's liveliness token is there, the daemon answering every rpc once, and fails with `Error::Gone` once the token goes, or at once when it isn't there. Every other rpc gives up after five seconds without an answer.
- **The tray is a second service.** It serves its notification switches through the daemon, and `TrayClient` is to it what `DaemonClient` is to the daemon: availability, a mirror of the switches, and rpcs to set them. `TrayClient::beside` shares a daemon client's session, and a `StateWatcher` given both merges their events. A tray is only ever seen while a daemon runs.

## Requirements

- **A tokio multi-thread runtime.** The client spawns tasks on the ambient runtime, which must be the multi-thread scheduler (zenoh, which carries the protocol, rejects the current-thread one). Construct it from inside that runtime.
- **Linux.** The daemon listens on a Unix socket in the user's runtime directory.

## Compatibility

A client talks only to a daemon of its own release line, the same `major.minor`: patch releases interoperate, and a pre-release such as `0.5.0-rc.1` works only with exactly itself. The client checks the daemon's release in each lifetime of the daemon, starting when the daemon appears and retrying until it gets an answer, and keeps only an answer got while the daemon's liveliness token is present. The answer is a mirror read, `service_info`, announced by `StateEvent::ReleaseChecked`. On a mismatch, rpcs fail with `Error::VersionMismatch`, the daemon client's mirror reads return the same error, the tray client's `notification_settings` returns `None`, and `StateEvent::VersionMismatch` fires, until the service restarts on a matching release.

## Durations

The client waits on four durations of its own, all inherited from TisVlay's SDK, whose mirror this is:

| Duration | What it bounds |
|---|---|
| 5 s | How long a daemon lifetime may hydrate before the mirror counts as hydrated with whatever arrived. zenoh doesn't say when replayed history is complete. |
| 100 ms to 5 s | The backoff between attempts to subscribe to the daemon's keys. |
| 1 s to 30 s | The backoff between release checks that got no answer, while the daemon stays up. |
| 300 s | The watcher's fallback tick, a safety net for a missed change; zero turns it off. |
