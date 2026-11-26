# zenoh pins an lz4_flex with a decompression advisory

lz4_flex before 0.11.6 can leak uninitialized memory, or a reused output buffer's old contents, when decompressing (RUSTSEC-2026-0041). The lockfile has lz4_flex 0.10.0, pulled in by `zenoh-transport` 1.10.2, the latest zenoh release, which requires 0.10. The fixed 0.11 line is semver-incompatible, so no bump reaches it until zenoh moves.

zenoh calls lz4_flex only behind its `transport_compression` feature (`src/common/batch.rs` in zenoh-transport). The workspace builds zenoh with `default-features = false` and `transport_unixsock-stream` alone (the root `Cargo.toml`), so the crate is compiled in but its compression and decompression never run. `packaging/licensing/deny.toml` ignores the advisory for that reason, so `just audit` passes. After TisVlay's concern of the same name.

**Risk.** None while compression stays off. Turning it on, directly or through a zenoh feature that implies it, would make the decompression reachable on every zenoh link, which here are the user's own Unix sockets.

**Mitigation.** Compression stays off. If a zenoh bump needs it, check the lz4_flex version first.

**Closes when.** A zenoh release depends on lz4_flex 0.11.6 or later and the workspace moves to it; the ignore entry goes with this file.
