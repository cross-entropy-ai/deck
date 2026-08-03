# Code architecture audit — 2026-08-02

Scope: the complete Rust workspace prepared for the `v0.11.1` release,
including application orchestration, session/system abstractions, background
workers, external-command boundaries, tests, and architecture documentation.

## Outcome

All findings in this audit are remediated in the working tree. No high- or
medium-severity item remains open. The post-remediation score is **9.4 / 10**.

| ID | Severity | Finding | Resolution and executable evidence |
|---|---:|---|---|
| AUD-01 | High | The remote attach prelude exposed a bare marker glob to the login shell; zsh could reject an unmatched glob before cleanup ran. | Cleanup now uses a quoted `find -name` pattern. `attach_cleanup_uses_find_pattern_not_a_shell_glob` locks the generated command. |
| AUD-02 | High | Create/list-directory failures, executor spawn failures, and backend panics could be collapsed into booleans/tuples or dropped. | `SessionControlResult<T>`, `DirListing`, and typed `OpOutcome` failures cover every operation. `SessionExecutor::submit` returns `Result`, retries a stale sender, and reports caught panics while keeping the FIFO alive. |
| AUD-03 | Medium | The System extension point still depended on global lookup, tmux session DTOs, and upper-layer host decoding. | `SystemRegistry` is injected into App and refresh; `SessionSnapshot`, `LaneSnapshot`, `SectionDef`, and `LaneId` cross the boundary. A fixture second System verifies sections, snapshots, and control without shell changes. |
| AUD-04 | Medium | Local and remote refresh had parallel result types and two large application paths with duplicated filtering, focus, ordering, and agent reconciliation. | Refresh is routed per lane and returns one `LaneRefresh`; foreground/background scheduling differs only inside the worker. App applies both through one lane merge path and rejects stale results after config reload. |
| AUD-05 | Medium | `dispatch.rs` mixed reduction, effect execution, and ad-hoc focus/probe thread creation. Spawn failures were silent. | Side-effect execution moved to `app/effect_runner.rs`; thread/channel ownership moved to `app/focus_executor.rs`; dispatch reports spawn failures through the in-UI warning path. |
| AUD-06 | Medium | `ssh -G`, `brew --prefix`, and port-forward control commands used unbounded `Command::output`. | All use the shared process-group-aware `CommandRunner` with explicit deadlines. Runner-injection tests verify SSH config and Homebrew detection. |
| AUD-07 | Low | System/session design documents described obsolete planning-stage APIs and understated the remaining attachment seam. | Both design documents now describe the implemented contracts, data flow, executor semantics, extension test, and intentional PTY compatibility boundary. |
| AUD-08 | Low | Two instance-lock tests could inspect a forked fixture before it completed `exec`, causing parallel-suite flakes. | The fixture now waits, with a one-second bound, until the exact process identity used by production is observable. |

## SOLID scorecard

| Principle | Score | Evidence |
|---|---:|---|
| Single Responsibility | 9.1 | Effect execution, focus execution, session execution, refresh orchestration, and backend adapters have distinct modules. `AppState` remains broad but its layout/focus logic is already split into submodules. |
| Open/Closed | 9.4 | Registry-driven lanes/sections/snapshots/control and the second-System contract remove switch growth for new backends. Full replacement of the embedded PTY transport remains a separate lifecycle extension. |
| Liskov Substitution | 9.6 | Local, remote, and fixture controls obey one result protocol; no implementation returns a success-shaped failure. |
| Interface Segregation | 9.5 | `SessionControl` is limited to the blocking control plane; PTY lifecycle and shell policy are not forced into backend implementations. |
| Dependency Inversion | 9.5 | App depends on `SystemRegistry`, `System`, `SessionControl`, and command-runner abstractions; concrete tmux/SSH decisions sit at composition or adapter boundaries. |

Additional engineering dimensions: reliability 9.5, testability 9.7,
documentation 9.3. Weighted overall: **9.4 / 10**.

## Reproduce the audit gates

Run from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git diff --check
```

The suite includes explicit coverage for typed mutation failures, directory
failures, executor spawn failure, backend panic recovery, background refresh
spawn/panic/single-flight behavior, second-System mounting, zsh-safe attach
cleanup, bounded SSH/Homebrew commands, and parallel-safe instance-lock
fixtures.

## Deliberate boundary and runtime validation

`SessionEntry.host` and the local/remote PTY manager remain a compatibility
seam for the existing tmux/SSH attachment transport. Generic backend identity,
layout, snapshot, control, and refresh routing no longer depend on concrete
tmux types. Generalizing terminal attachment itself should be a separate
feature driven by a real second display transport, not speculative interface
surface.

This audit executes compile-time and automated workspace gates. It does not
claim a live remote zsh/SSH/tmux smoke test; that requires a configured external
host and should remain a release/manual acceptance check.
