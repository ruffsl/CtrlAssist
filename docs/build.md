# Build system

How the workspace compiles, which checks gate a change, and the lint policy. The policy and most of the reasoning are TisVlay's, where they were arrived at by measurement; what is recorded here is what applies to this workspace so far. A check, tool or workflow is described here from the change that adds it.

## Gates

Every project command is a `just` recipe, and each prek hook runs the recipe of the same name.

| Recipe | Checks |
|---|---|
| `just deny` | Licenses against the allow-list, one version of each crate, everything from crates.io |
| `just shear` | No unused or misplaced dependencies |
| `just typos` | Spelling, American English |
| `just fmt` | rustfmt, and tombi for TOML (formatting only: it doesn't sort, since the manifests group dependencies under comments) |
| `just build` | The workspace builds: test profile, all targets, all features |
| `just clippy` | Clippy, warnings denied |
| `just doc` | API docs, warnings denied: a broken intra-doc link fails |
| `just test` | nextest, then doc tests |
| `just actionlint` | The workflows: syntax, expressions, and shellcheck over their `run:` lines |
| `just zizmor` | The workflows and Dependabot config for security problems |
| `just licenses-html` | The third-party license bundle the packages ship can be written: it fails on a crate whose license `packaging/licensing/about.toml` doesn't accept |
| `just check` | All of the above |

Build and clippy use the test profile, so the tests reuse what they compiled.

## CI

`ci.yml` runs on pushes to `main` and on pull requests, and dispatches through the same recipes, so a failure reproduces locally.

| Job | Runs | Locally |
|---|---|---|
| `checks` | File checks, spelling and the workflow linters; on every run, drafts included | `prek run -a` |
| `changes` | Decides whether the change touches code or only docs | |
| `ci` | Formatting and docs hooks, then clippy and the tests under one coverage-instrumented build | `just test-ci` |
| `msrv` | The workspace builds on its oldest supported compiler | `just msrv` |
| `docs` | The user guide's generated pages are current, and the guide builds with its local links and anchors intact | `just docs-gen-check`, `just docs-links` |
| `CI result` | Passes only if the others passed; the one check a branch ruleset requires | |

- **Drafts run only `checks`, and docs-only changes `checks` and `docs`;** marking a pull request ready starts the rest. Docs-only means `docs/`, `.claude/`, the README, the license, and the contributing guide, security policy, pull request template and issue forms under `.github/`.
- **Tools whose output can change between releases are pinned to the dev shell's versions** in the workflows (typos, zizmor, shellcheck, actionlint, cargo-about, cargo-deny, cargo-shear, tombi, lychee, mdbook and the packaging tools), so local and CI agree. Bump them together after a `nix flake update`.
- **One instrumented build.** `just test-ci` routes clippy, nextest and the doc tests through cargo-llvm-cov's environment, so they share a target directory instead of compiling twice.
- **Coverage is also sent to Codecov,** for its trend and a pull request's diff annotations. The upload authenticates with GitHub's OIDC token, so no secret is stored, and an upload that fails warns and never fails CI: the floor is the gate.
- **Coverage has a floor,** `cov_baseline_lines` in the justfile. `just test-ci` fails below it. Raise it when coverage rises (`just cov` prints the figure); never lower it. It was set once, to 91, at what CI first measured: the 95 before it was written before the settings window was, and never run in CI until the rewrite's pull request left draft. Kept out of the figure, as `cov_exclude` in the justfile lists them: `ctrlassist-test`; the hiding service's files that need its installed user and udev rule, which only the virtual machine checks can run; and the tray's `desktop.rs` and `service.rs`, which need a desktop's session bus and hold only what puts the rest on a desktop. `just cov-html` and `just cov-lcov` write the same figure as a report, and `just cov-ratchet` checks it against the floor without the gates around it; all of them run every test and say afterwards if one failed.
- **Actions are pinned to a commit,** with the release in a comment, and Dependabot proposes updates to both after a one-week cooldown. To pin a newly added action, run `GITHUB_TOKEN=$(gh auth token) pinact run`.

## Toolchain

`rust-toolchain.toml` pins an exact stable release, so a build reproduces from a commit alone; one file drives rustup and the Nix dev shell. `rust-version` in `Cargo.toml` is the oldest compiler the workspace supports and may trail the pin. It is raised only when an update needs it, and never past the rustc that nixpkgs ships, because the Nix package builds with the consumer's compiler.

## Profiles

| Profile | For | Notable settings |
|---|---|---|
| `dev`, `test` | Local work and the gates | Split debug info; `test` keeps line tables only |
| `release` | People compiling locally | Stripped; no LTO, so it stays quick |
| `dist` | The packages and the flake's package | Thin LTO, one codegen unit, optimized for size; unwinds on panic |

Every profile unwinds on panic. The input thread catches a panic in one setup's step and tears down that setup alone, leaving the others' virtual pads; `panic = "abort"` would turn that into the daemon exiting with every virtual pad gone. TisVlay's `dist` profile aborts, and measured that as 14% smaller binaries; here the isolation is worth the size.

## Lint policy

### Warnings are errors in the gates

Every gate recipe denies warnings by exporting `CARGO_BUILD_WARNINGS=deny` (cargo's `build.warnings`). So a lint at `warn` in `[workspace.lints]` blocks a commit just as `deny` would.

- **Why `build.warnings` and not `-- -D warnings`:** it covers clippy, test and rustdoc builds with one setting, fails on warnings replayed from the cache, and toggling it doesn't invalidate the build cache.
- **Why per recipe and not `.cargo/config.toml`:** the Nix package build reads that file with nixpkgs' rustc, and a warning a newer compiler adds would then break a rebuild. Local `cargo build` and `cargo run` stay warn-only, so a half-edited file still runs.

### Adding a lint

1. **Pick lints, never enable groups.** No `pedantic`, `nursery`, `restriction` or `clippy::cargo` group is enabled; lints enter the `[workspace.lints]` table one at a time, on their merits.
2. **Strict by default, exceptions opt out.** A lint goes in the shared table, so every crate inherits it. A crate or file that legitimately differs opts out with one reasoned attribute.
3. **Every allowance states its reason.** `#[expect(lint, reason = "…")]` where the lint fires unconditionally, so a stale allowance fails the build. `#[allow(lint, reason = "…")]` only where it depends on features or tests.
4. **A new lint lands at zero sites.** Its change fixes or expects every site; there is no "fix later" marker.
5. **Prove it red.** Before review, a deliberate violation must fail `just clippy`.
6. **`unsafe` is confined.** `unsafe_code` is `deny`, not `forbid`, so a file that needs it opts out with a reasoned file-level `#![expect(unsafe_code)]`. Every block gets a `// SAFETY:` comment and does one unsafe operation.
7. **Test code may unwrap, panic, index and print.** `.config/clippy.toml` sets the matching keys. They cover `#[test]` functions and `#[cfg(test)]` modules, not helper functions under `tests/`, so an integration-test root carries a reasoned `#![expect]`.
8. **A tool that joins `just check`** comes from the flake dev shell and gets a prek hook whose id matches its recipe name.

### Parsers of outside input

Code that handles bytes or text from outside the process (a controller database, a HID report, a profile file, a line of sysfs) carries a module-level profile: `#![warn]` of `indexing_slicing`, `arithmetic_side_effects`, `string_slice` and `cast_possible_truncation`. Each site is then a checked access, a typed refusal, or a reasoned `#[expect]` stating the bound. An entry point that parses raw input also gets a "never panics on arbitrary input" property test.

This matters more here than in most programs: a panic on the input thread takes every virtual pad with it.

### Never adopted

Recorded so nobody re-scouts. TisVlay's build notes have the measurements.

- `nursery`, `restriction` and `clippy::cargo` as groups.
- `expect_used` (an `expect` stating its invariant is the idiom here), and `indexing_slicing`, `arithmetic_side_effects` and `as_conversions` workspace-wide.
- `allow_attributes` (it flags reasoned allows), `redundant_pub_crate` (it contradicts `unreachable_pub`), `missing_docs_in_private_items`.
- A nightly toolchain or nightly-only rustfmt options.

## Dependencies

Every dependency is declared once in `[workspace.dependencies]` and inherited, but for `slint-build` and `i-slint-backend-testing`, which are pinned exactly to Slint's release in the one crate each that uses them, as in TisVlay; the three move together. The core crate has none. zenoh, the control plane, brings most of the tree and every exception in `packaging/licensing/deny.toml`; its pin and feature set are TisVlay's, and the two move together.

A crate that needs a fix upstream doesn't have yet is carried under `vendor/` and wired in with `[patch.crates-io]`; [vendor/README.md](../vendor/README.md) lists each one and its fix. The controller database snapshot in `crates/ctrlassist-device/data` is data under the zlib license, kept beside it; `just update-controller-db` refreshes it from upstream at a named commit, and its diff is reviewed like code.

Vendored code keeps upstream's formatting and is outside the workspace's lints, the typos gate and the coverage figure, so its diff against upstream is the fix alone.

`just deny` holds the graph to one version of each crate. When an update brings a second version of something, first try to move the crates that pin the old one; only if upstream forces the duplicate does it go in `packaging/licensing/deny.toml`'s skip-list, with its reason. A skip that no longer matches anything fails the gate, so the list can't outlive what it excuses.

`just audit` checks the RustSec advisory database. It is not part of `just check`: advisories change without the lockfile changing, so they are checked on their own schedule rather than gating unrelated work. An advisory with no upgrade that reaches it is triaged into `packaging/licensing/deny.toml`'s ignore list, each entry with its reason and the dependency path it arrives by; one that is fixed upstream but held back gets a doc under `docs/concerns/` too.

### Review triggers

Things to look at again when the stated condition changes.

| Watch | Because | Revisit when |
|---|---|---|
| `evdev` | The crate chosen for reading pads and creating uinput pads had gone over a year without a release when it was picked, though its repository was active. `evdevil` is a newer alternative with force feedback, hotplug and async built in. | A needed fix sits unreleased. |
| `gilrs` | Not used: it opens pads by path, owns its own polling, and keeps its controller-database parser private. | It gains a way to take a descriptor and lend its mappings. |
| uhid bindings | The uhid events are laid out by hand, because `uhid-virt`'s FFI layer runs bindgen and needs libclang at build time. | A binding without that cost appears. |
| `zbus` | `ksni` holds the tray at zbus 5. | `ksni` moves to a newer zbus. |
| InputPlumber | GPL-3.0-or-later. It can be studied and spoken to, never copied from, in an Apache-2.0 project. | Never; this is a standing rule. |
| zenoh and zenoh-ext | zenoh-ext is pinned with a tilde (`~1.10`) because the SDK's mirror rides on its `unstable` API (the advanced subscriber and its history), which may break within 1.x; both pins are TisVlay's. | zenoh 2.x, or any zenoh-ext bump: move the pin deliberately, with TisVlay's, checking the advanced subscriber the SDK uses. |
| `usvg` | The tray reads a desktop theme's icon files with it, at the release Slint's own SVG support brings in, so the graph has one. The tray takes its `svgz` and `writer` features and none of its text ones. | Each Slint bump: move the pin to the release Slint then uses, and look again at the icons the tray composes. |
| winit's SCTK 0.19 path | winit 0.30, which Slint's `unstable-winit-030` backend uses, forces the SCTK 0.19 entries in `deny.toml`'s skip-list. | Each Slint or winit bump; one that moves winit to SCTK 0.20 drops them. |
| Slint's `Focusable` init | `Focusable` runs `focus-in` from `init` for a control created already focused, because Slint doesn't run the `has-focus` change handler for a control created and focused in one pass (slint-ui/slint#14894). | A Slint release with its fix, slint-ui/slint#14927: drop that `init` and the notes on it in [gui/remote-navigation.md](gui/remote-navigation.md); the navigation tests cover the created-and-focused case. |
| `packaging/nix/slint-lsp.nix` | The editor's slint-lsp has to read the workspace's Slint edition, and nixpkgs' is older. | nixpkgs' slint-lsp reaches the Slint pin: delete the file and use nixpkgs' package in the dev shell. |
| tombi | The flake overrides nixpkgs' tombi to the version CI installs, so local and CI formatting agree. | nixpkgs reaches the pinned version: drop the override. Bump the pin and CI's `tombi@<version>` together. |
| `LLVM_PROFILE_FILE` in `.cargo/config.toml` | Defaulted so a test binary left instrumented by an earlier coverage run writes its data under `target/profraw/`, not into each crate's directory. Cargo leaves this to the user. | rust-lang/cargo#13040 or #13136 progress. |
| cargo-shear | It finds unused and misplaced dependencies, which cargo may learn to do itself. | The toolchain pin reaches the release that stabilizes cargo's own lints: try `[workspace.lints.cargo]` `unused_dependencies` and `missing_lints_inheritance`, and if they cover what shear finds, retire `just shear`. |
| clippy's test allowances | `.config/clippy.toml` allows lints in tests one key at a time, and they don't reach helpers under `tests/`, so each integration-test root carries a reasoned `#![expect]`. | clippy's generic `allow-in-tests` (rust-clippy#17520) ships: replace the per-lint keys, and delete the roots' `#![expect(clippy::unwrap_used)]`. |
| The pedantic group | No lint group is ever enabled (see the lint policy), so new lints worth having arrive only when looked for. | Once a year: run clippy once with `-W clippy::pedantic`, and the nursery lints worth a look, and consider new hand-picked lints for `[workspace.lints]`. |
| actionlint and `$/` | zizmor asks for GitHub's `uses: $/…` self-repository syntax, which actionlint doesn't accept yet, so `release.yml`'s call to `build-packages.yml` carries a zizmor ignore. | actionlint accepts `$/`: switch the call over, drop the ignore, and check that the next tag's attestations still name `build-packages.yml` as the signer. |
