# Done Certificate — Task 06: Core port traits and dependency direction

**Task:** [06-core_port_traits.md](06-core_port_traits.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 06. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 06) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the port traits every adapter implements, defined in `lotta-runtime` so the runtime never links a concrete adapter crate.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not introduce a runtime→adapter edge. `architecture-principles.md` §Dependency graph states `lotta-runtime` depends on domain and port traits, never concrete adapters, and forbids circular crate dependencies.

## Obligations

- **O1 — Every I/O effect named in `architecture-principles.md` §Architectural pattern is expressed as a port trait method in `lotta-runtime::ports`**
  - *Claim:* Filesystem, network, process, clock, random, Git, and provider effects each have a trait method, and none is performed inline in the runtime.
  - *Evidence to collect:* Read `crates/lotta-runtime/src/ports/mod.rs` and map each of the seven effect classes to at least one trait method. Grep `crates/lotta-runtime/src/` outside `ports/` for `std::fs`, `std::process`, `reqwest`, `SystemTime::now`, and `rand::` — expect zero matches.
  - *Status:* ☐ unverified

- **O2 — `lotta-runtime` declares no dependency on any adapter crate, and the assertion is enforced by a test rather than by review**
  - *Claim:* `cargo metadata` shows `lotta-runtime`'s dependency set excludes all seven adapter crates, and a test fails if one is added.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(ports::dependency_direction)'` — expect PASS. Read the test and confirm it parses `cargo metadata` output (or `Cargo.toml`) and asserts the forbidden-dependency list from `architecture-principles.md` §Dependency graph.
  - *Checks:* Resolve which crate defines each port trait — confirm it is `lotta-runtime::ports`, not `lotta-providers` or `lotta-tools`. `architecture-principles.md` §Dependency graph permits adapters to depend on runtime interfaces but forbids the runtime depending on concrete adapters; a port trait co-located with its adapters inverts that edge.
  - *Status:* ☐ unverified

- **O3 — Every streaming port method carries an explicit cancellation token and a bounded channel; no unbounded channel appears in a port signature**
  - *Claim:* Each streaming method takes a cancellation token parameter and returns a bounded stream or writes to a bounded sender.
  - *Evidence to collect:* Grep `crates/lotta-runtime/src/ports/` for `unbounded_channel` and `unbounded` — expect zero matches, per `development-guidelines.md` §Async and concurrency. Read each streaming trait method signature and confirm a `CancellationToken` parameter is present.
  - *Status:* ☐ unverified

- **O4 — Each trait's rustdoc states its preconditions, error type, cancellation behavior, and ownership**
  - *Claim:* `cargo doc` renders every public port item with the four documented properties and no missing-docs warning.
  - *Evidence to collect:* Run `RUSTDOCFLAGS='-D warnings' cargo doc -p lotta-runtime --no-deps` — expect exit code 0. Spot-read three trait docs and confirm each names preconditions, the error type, cancellation behavior, and who owns the returned value.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(ports::)'` and sees the dependency-direction assertion and port-signature checks pass, then confirms `cargo tree -p lotta-runtime` lists no adapter crate**
  - *Claim:* The ports test module passes and the dependency tree contains no adapter crate.
  - *Evidence to collect:* Run the filter (expect zero failures), then run `cargo tree -p lotta-runtime --edges normal` and confirm none of the seven adapter crate names appears.
  - *Status:* ☐ unverified

## Regression check

No existing callers in scope — this task introduces new units that nothing else calls yet, and modifies no unit produced by a task it depends on.

## Residue

`ProviderPort` and `ToolPort` are defined in Tasks 07 and 08 in the same module, because their shapes are specified on separate pages. The composition root that injects concrete adapters is Task 85.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
