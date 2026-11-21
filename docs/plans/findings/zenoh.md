# Finding: zenoh beside the input loop

Measured 2026-11-21 on the development host with `spikes/zenoh/` and the loop spike's bench, using zenoh 1.10.2 with only its Unix-socket transport, as TisVlay pins it. Reproduce with `just spike-zenoh-bench`.

## Verdict

**zenoh stays, with real-time scheduling doing more work than first thought.** On an idle host, and under load with real-time scheduling, a zenoh session in the same process does not disturb the input thread. Under saturating load without real-time scheduling the zenoh daemon had bad runs more often than the loop alone.

The plan's rule was that the input thread's latency and wakeup numbers fall within the run-to-run spread the loop spike recorded. They do in every condition but that last one (below).

## Setup

The loop spike's input loop runs on its own thread, created before anything else. The same process runs a tokio runtime with two workers and a zenoh router listening on a Unix socket, with a status publisher, one rpc, a liveliness token, and a mirror of what the loop forwards. One client is attached and idle throughout; a second subscribes to the mirror in the third case. The bench is the loop spike's: 1,000 stick frames a second on one fake pad and 1,000 button edges on the other, for ten seconds, three runs per row.

## Measurements

| Condition | Case | p50 | p99 | max | Input thread idle wakeups/s | Whole daemon idle wakeups/s | CPU | Memory |
|---|---|---|---|---|---|---|---|---|
| Idle host | loop alone | 21–33 µs | 65–147 µs | 242–325 µs | 0 | 0 | 1.8–2.6% | 3 MB |
| | zenoh, idle client | 25–32 µs | 122–134 µs | 289–1,066 µs | **0** | 2.6–2.8 | 2.0–2.7% | 11 MB |
| | zenoh, mirror subscribed | 24–28 µs | 109–128 µs | 285–304 µs | **0** | 3.8–4.8 | 7.4–7.9% | 11 MB |
| Idle host, real-time | loop alone | 21–29 µs | 32–61 µs | 151–182 µs | 0 | 0 | 1.5–2.5% | 3 MB |
| | zenoh, idle client | 22–30 µs | 35–50 µs | 253–309 µs | **0** | 2.6–3.2 | 1.8–2.7% | 11 MB |
| | zenoh, mirror subscribed | 32–33 µs | 43–70 µs | 270–312 µs | **0** | 4.6 | 9.0–9.5% | 11 MB |
| Loaded, real-time | loop alone | 6 µs | 9 µs | 26–28 µs | 0 | 0 | 0.6% | 3 MB |
| | zenoh, idle client | 5–6 µs | 8–9 µs | 24–34 µs | **0** | 2.6 | 0.5–0.7% | 11 MB |
| | zenoh, mirror subscribed | 6–7 µs | 8–12 µs | 42–192 µs | **0** | 3.6–4.0 | 2.0–2.9% | 11 MB |

No frame was lost in any run.

- **The input thread never wakes when idle,** with zenoh in the process or not. The rest of the daemon wakes about three times a second with a client attached, which is zenoh keeping its link alive. Legacy woke about a hundred times a second.
- **Latency is unchanged** on an idle host and under load with real-time scheduling.
- **Memory goes from 3 MB to 11 MB** resident.
- **The stripped binary grows from 1.1 MB to 10 MB.** zenoh and tokio are nearly all of the difference.

### Loaded, without real-time scheduling

This is the one case that is not within the loop's own spread. Counting every run of this condition, the first three and eight more:

| Case | Runs | Runs with p99 above 100 µs | p99 in those runs |
|---|---|---|---|
| loop alone | 11 | 1 | 932 µs |
| zenoh, idle client | 11 | 3 | 829, 830, 832 µs |
| zenoh, mirror subscribed | 3 | 2 | 881, 1,004 µs |

In one of the zenoh runs the median itself was 824 µs: the input thread spent the whole run a scheduling slice behind. With every CPU saturated and no real-time priority, the input thread is at the scheduler's mercy either way, but the numbers suggest it fares worse with zenoh's threads in the process. Eleven runs is too few to say how much worse.

What is not in doubt is the remedy: with real-time scheduling the zenoh daemon's worst frame in 30,000 was 34 µs. So real-time scheduling matters more for the daemon than the loop finding implied, and a machine that refuses it should be told plainly in the daemon's status.

## The mirror

Publishing what the loop forwards, for a client to draw a pad's live state:

- **While nobody subscribes it costs the input thread one flag check per frame.** The publisher's matching status sets the flag, so the daemon knows whether anyone is listening without polling.
- **While somebody subscribes,** the input thread offers each frame with a send that never waits and drops if the control plane is behind. Latency stayed in the same range; the daemon's CPU rose from about 2% to about 8% at 1,000 frames a second, nearly all of it on the control plane's threads publishing every frame.

That CPU cost is avoidable: a drawing client doesn't need every frame. The daemon should publish the latest state when the previous publish has completed, skipping what arrived in between, which bounds the cost by what the subscriber can take rather than by the pad's rate. That needs no timer.

## A one-shot client

A CLI verb that opens a session, makes one rpc and exits, three runs: the session opened in 3.0–3.6 ms, the reply arrived by 4.9–6.2 ms, and the process had closed its session by 5.5–6.9 ms. That is from inside the process, so it excludes loading the binary.

## Notes for the build

- **The input thread is created before the runtime and shares nothing with it** but the mirror's flag and channel: no lock, no task, no allocation on its side. Keep that boundary.
- **zenoh needs tokio's multi-thread scheduler,** as TisVlay found. Two workers were enough here.
- **The spike's zenoh code compiled and ran on the first attempt** by following TisVlay's bootstrap (scouting off, a `unixsock-stream` endpoint, router and client modes), which is some evidence that the patterns carry over as hoped.
- **Not exercised:** zenoh's behavior across suspend and resume, which TisVlay's architecture notes as a source of dropped links, and more than two clients.

## What this changes in the plan

- zenoh for the control plane is confirmed, with the caveat above.
- The mirror publishes the latest state when the last publish completes, not every frame.
- The daemon's status reports whether real-time scheduling was granted, since without it the tail under load is not dependable.
