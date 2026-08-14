# Development Guidelines

**Status:** Draft · **Date:** 2026-08-14 · **Owner:** Ant Stanley · **Scope:** Repo-wide

These are the rules of the road for Lotta. The project uses Rust and adopts Tiger Style: safety, performance, and developer experience, in that order. The repository is greenfield, so toolchain gates named here are acceptance requirements; [README.md](README.md) records that their configuration is not implemented yet.

---

## Toolchain

| Tool | Version / channel | Required use |
|---|---|---|
| Rust | stable, pinned | `rust-toolchain.toml`; edition 2024 |
| Cargo | pinned with Rust | workspace build and test entry point |
| rustfmt | pinned component | `cargo fmt --all --check` |
| Clippy | pinned component | all targets/features, warnings denied |
| cargo-nextest | pinned in CI image | fast and slow test profiles |
| cargo-deny | pinned in CI image | license, advisory, duplicate, and source policy |
| cargo-audit | pinned in CI image | release advisory check |
| rustdoc | pinned component | public API documentation with warnings denied |

The lockfile is committed. Build scripts, proc macros, and native dependencies receive the same dependency and security review as runtime code.

---

## Tiger Style — the pervasive style

This project adopts **Tiger Style** as its pervasive coding style. Deviations require a written reason in the change description.

The short form is: be defensive and validate everything. Assume any input not produced by the current function is wrong. Assume any invariant not asserted can be violated. Make every limit explicit, every error handled, and every assumption checked.

Priorities are **safety, performance, developer experience, in that order**. When they conflict, safety wins.

Load-bearing principles:

- **Zero deliberate technical debt.** A known shortcut is not merged without an accepted change spec that owns its removal.
- **Simple, explicit control flow.** No recursion. Non-trivial iterator/combinator chains give way to loops and matches that show branches.
- **Limits on everything.** Every loop, queue, retry, cache, parser, payload, and child process has an upper bound.
- **Assertions are first-class.** Core functions average at least two meaningful assertions covering preconditions, postconditions, or invariants.
- **Comments explain why.** They record constraints and rationale, not a paraphrase of the next line.

---

## Defensive coding and assertions

### Where to validate

| Boundary | Validation |
|---|---|
| HTTP/WS request → protocol | frame/body size, JSON depth, discriminant, IDs, enums, cardinality |
| Protocol → runtime | runtime existence, state transition, request correlation, lease ownership |
| Runtime → adapter | typed port preconditions and bounded request size |
| Adapter → runtime | vendor response status, shape, size, ordering and correlation |
| Disk/Git → domain | schema version, UTF-8, checksums, path confinement, entity invariants |
| Child/sidecar → host | frame length, protocol version, owner identity, capability and timeout |

Validation happens before allocation proportional to external values. Data is revalidated on disk read even when Lotta wrote it.

### Assertions in Rust

- Use `assert!`, `assert_eq!`, and `debug_assert!` throughout core code. Production `assert!` remains enabled.
- Core functions average two or more meaningful assertions. Constant truths and duplicate checks do not count.
- Pair important invariants at independent boundaries, such as validate-before-write and validate-after-read.
- Split compound assertions so diagnostics identify the violated property.
- Use compile-time assertions for layout and constant relationships.
- Production paths do not use `unwrap()` or `expect()`. Initialization-only `expect()` includes a reason proving prior validation.
- `panic!` signals programmer error only; it is not control flow.

### Errors are data, not exceptions

- Each crate exposes one typed error enum and translates vendor errors at adapter boundaries.
- Every error is handled or explicitly propagated. Empty catch/log-and-continue behavior is forbidden.
- Stable error codes survive refactors and client upgrades.
- Retry policy names max attempts, delay function, retry-after handling, retryable kinds, and total deadline. Jitter appears only when the owning component spec requires it.
- Secret-bearing types redact `Debug` and `Display`; errors pass through scrubbing before logs or wire output.

### Make invalid states unrepresentable

- Use newtypes for IDs, bytes, durations, tokens, paths, revisions, and sequence numbers.
- Use exhaustive enums for lifecycle, protocol variants, stop reasons, tool outcomes, and provider events.
- Use validated path and non-empty string types at trusted boundaries.
- Use bounded collection wrappers where cardinality is part of the contract.
- Do not use boolean clusters to model state machines.

---

## Limits and bounds

Every limit is a named constant with units last, for example `frame_bytes_max`, `retry_delay_ms_max`, and `queue_items_max`. Concrete server values live in the relevant component spec.

Reaching a limit is observable through a structured event and counter. A hard limit rejects input or applies backpressure; it never silently drops. A documented soft coalescing limit may replace older coalescable work only while emitting the baseline drop reason and authoritative queue snapshot. Every loop either iterates a bounded collection or asserts measurable progress toward a bound.

Configuration can lower a safety maximum. Raising a hard maximum requires a reviewed spec change and boundary tests at one below, equal to, and one above the new value.

---

## Version control: Git

- Commits are small and coherent. Stage specific paths; do not sweep unrelated files.
- Subjects follow Conventional Commits: `type(scope): subject`.
- Commit messages and pull requests explain why and name architecture-level effects.
- `main` remains releasable; feature work uses named branches/worktrees.
- Published history is not rewritten without explicit owner approval.
- Hooks are not bypassed. A failed hook is fixed, not skipped.
- Destructive operations require explicit human confirmation.

The Git repository and hooks are not present yet; [README.md](README.md) tracks this greenfield divergence.

---

## Rust conventions

### Formatting and linting

- `cargo fmt --all --check` is clean before review.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` is clean.
- `clippy.toml` enables pedantic-adjacent lints. Every opt-out carries a why comment.
- Workspace crates use `#![deny(missing_docs)]` for public library APIs and `#![forbid(unsafe_code)]` unless an approved adapter exception applies.

### Code style

- Hard limit: 70 lines per function. Extract pure helpers and keep orchestration linear.
- Hard limit: 100 columns per line.
- No recursion. Use iteration with an explicit bound.
- Prefer modules and responsibility-sized files; a 1,000-line `.rs` file is split.
- No business logic in `main.rs`, handlers, or Serde conversion code.
- Use fixed-width integers for domain/wire values. `usize` does not cross serialization boundaries.
- Errors are `Result` with `thiserror`; vendor errors translate before entering core code.
- No `unsafe` outside approved OS adapters. Every block has a `// SAFETY:` proof and invariant tests.
- Add `#[must_use]` to builders and values whose ignored result would be a bug.
- Prefer explicit `match` over chains hiding non-trivial branches.
- Pass large non-moved values by reference.
- Calculate values near use; mutable state has one owner.
- State invariants positively where possible.

### Naming

- `snake_case` for functions, variables, modules, files; `CamelCase` for types and traits.
- Acronyms use proper case: `HttpClient`, not `HTTPClient`.
- Avoid abbreviations beyond established `id`, `cfg`, `ctx`, and loop counters.
- Units come last: `latency_ms_max`, not `max_latency_ms`.
- Related names use parallel forms such as `source` and `target`.
- Private helpers prefix with the parent operation when that makes call history discoverable.
- Callback/closure parameters appear last.

### Async and concurrency

- Every spawned Tokio task has an owner, cancellation token, bound, and join path.
- No lock guard lives across `.await`.
- Bounded channels are the default; unbounded channels are forbidden.
- Conversation state has one mutating owner.
- Blocking filesystem, Git, crypto, and child-process waits use explicit blocking pools with concurrency bounds.
- Timeouts wrap external I/O; timeout errors remain distinct from cancellation.

### Testing

- `cargo nextest run --workspace --all-features` runs the fast tier.
- In-module tests cover pure logic. Crate `tests/` cover ports with in-memory adapters. End-to-end tests run the assembled binary.
- Every success path has negative-space tests for defined failures.
- Every limit has below/at/above tests.
- `proptest` covers parsers, state machines, path normalization, transcript chains, and protocol round trips.
- Tests use fake clocks and deterministic IDs. Wall-clock sleeps and public network dependencies are forbidden.
- A flaky test is a correctness bug and blocks release.

### Documentation

- Public items in library crates have rustdoc describing invariants, errors, cancellation, and ownership.
- Each crate root documents its responsibility, ports, dependencies, and forbidden dependencies.
- `TODO` includes an owner and issue/change-spec reference. Bare TODOs are forbidden.

---

## Repository hygiene

- `.specs/` is canonical for design and decisions.
- `fixtures/` contains sanitized, reviewable compatibility data; no credentials or user transcripts.
- Generated protocol/schema output is checked in and CI proves regeneration is clean.
- Local state, provider credentials, build output, coverage, and temporary sidecar data are ignored.
- Dependencies are minimal, pinned, licensed, and audited. Duplicate foundational crates require justification.
- Source files do not contain dead compatibility branches. Compatibility belongs in named adapters and fixtures.

---

## Guidelines for AI agents

1. Read `.specs/README.md` and the owning component page before editing.
2. Stay inside dependency direction; core code does not gain I/O for convenience.
3. Add assertions and explicit bounds in the same change as new control flow.
4. Do not invent wire fields or discriminants. Update fixtures/spec/schema first.
5. Test positive and negative space together.
6. Never swallow errors or expose vendor errors from core crates.
7. Do not add backwards-compatibility shims outside a named compatibility adapter.
8. Do not use `unwrap`, `expect`, unbounded channels, recursion, or wall-clock sleeps in production paths.
9. Run formatting, Clippy, unit, compatibility, and affected integration tests before claiming completion.
10. Do not commit secrets, real transcripts, provider payloads, or unsanitized traces.
11. Do not perform destructive VCS operations or skip hooks without explicit permission.
12. Update the spec when a load-bearing decision changes.

---

## Definition of done

A change is done when:

- behavior has unit/integration/end-to-end coverage at the right boundary,
- negative-space and validity-boundary tests exist,
- touched core functions meet assertion and function-size rules,
- every new bound is a named constant and observable,
- error and cancellation paths are explicit,
- `cargo fmt`, Clippy with denied warnings, rustdoc, and nextest pass,
- protocol/domain changes update schemas and compatibility fixtures,
- persistence changes pass TypeScript↔Rust round-trip tests,
- `cargo deny` and advisory checks pass for dependency changes,
- specs and architecture pointers remain accurate,
- the change description states why and lists architecture-level effects.

---

## Assumptions and open questions

**Assumptions**

- Rust is the only language in the server workspace; JavaScript runs only in external compatibility hosts.
- CI can execute the pinned TypeScript reference and existing JavaScript clients for conformance.

**Decisions**

- *Pervasive style.* **Tiger Style.** Long-running agent infrastructure needs explicit limits, assertion-heavy invariants, and safety-first tradeoffs.
- *Error model.* **Typed `Result` values.** Errors participate in protocol, persistence, retry, and recovery behavior.
- *Function size.* **70 lines maximum.** Runtime and protocol functions must remain reviewable as complete control-flow units.
- *Version control policy.* **Git with Conventional Commits.** This matches the adjacent reference repositories and worktree-oriented development.

**Open questions**

- *Gate implementation.* Which CI provider and hook manager wire the required checks?
- *Coverage policy.* What branch/line thresholds supplement behavior-based conformance gates?
