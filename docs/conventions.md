# Code conventions

<!-- Imported by .claude/CLAUDE.md, so every line here costs context in each Claude session. Keep it short. -->

These are the rules the lints can't fully express. The lint policy and its reasons are in [build.md § Lint policy](build.md#lint-policy).

- **Panics state their invariant.** `expect("… because …")`, never a bare `unwrap()` in shipped code. Tests may unwrap.
- **Nothing on the input thread can panic from outside input.** A profile, a pad's report, a controller-database line and a game's request are all checked before they reach it; building a graph rejects what a step couldn't handle.
- **Errors:** library crates expose typed errors callers can match on, `#[non_exhaustive]`. Binaries and their glue may use `anyhow`.
- **Logging:** through `tracing`, with structured fields rather than values formatted into the message, and never from the input thread. No `println!` or `eprintln!` anywhere (lint-enforced): they panic on a closed pipe, and a panic in the daemon drops every virtual pad. Command output is written with `writeln!` and the error handled.
- **Handles whose drop stops work** (a running setup, a hidden pad, a virtual pad) carry `#[must_use = "…"]` saying what dropping them does.
- **Allowances:** reasoned `#[expect(lint, reason = "…")]`. Use `#[allow]` with a reason only where the lint depends on features or tests.
- **`unsafe`:** only in files that carry a reasoned `#![expect(unsafe_code)]`, one operation per block, each with a `// SAFETY:` comment.
- **No new timeouts.** A duration that isn't the user's or the game's own is a design smell; look for the state or event that makes it unnecessary.
- **One word per thing,** from the Terms table in [architecture.md](architecture.md#terms).
- **Spelling and wrapping:** American English, which the typos gate enforces. Markdown isn't hard-wrapped: one paragraph per line.
