# Done Certificate — Task 17: Turn lifecycle owner and lease-guarded effects

**Task:** [17-turn_lifecycle_owner_and_leases.md](17-turn_lifecycle_owner_and_leases.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 17. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 17) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** one lifecycle owner per scope that issues leases and suppresses every effect from a stale lease at each awaited boundary.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not let a stale lease release a newer turn: `03-runtime-and-turns.md` §Lifecycle owner makes this the load-bearing guarantee behind exactly-once terminal events.

## Obligations

- **O1 — The lifecycle owner is the only writer of `TurnState`, and every transition out of `Idle` mints a new lease generation**
  - *Claim:* No other module mutates `TurnState`, and lease generations strictly increase per scope.
  - *Evidence to collect:* Grep `crates/lotta-runtime/src/` for assignments to the turn-state field outside `lifecycle.rs` — expect zero. Run `cargo nextest run -p lotta-runtime -E 'test(lifecycle::lease_generation_increases)'` — expect PASS.
  - *Status:* ☐ unverified

- **O2 — Every awaited boundary re-checks all four conditions, and a stale lease writes nothing, emits nothing, and releases nothing**
  - *Claim:* An effect captured under generation N, resumed after generation N+1 owns the scope, performs no persistence write, no emission, and does not release the newer turn.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(lifecycle::stale_lease_suppression)'` — expect four cases: `no_persistence_write`, `no_emission`, `does_not_release_newer_turn`, and `respects_cancellation_policy`. Trace: lease N captured → await → lease N+1 installed → resume → assert fake store write count is 0 and outbound message count is 0.
  - *Checks:* Resolve the lease-currency check inside each awaited path — confirm it compares against the runtime's live generation, not a value captured alongside the lease. `NAME SHADOWING` risk: a local `lease` binding must not shadow the runtime's current lease accessor.
  - *Status:* ☐ unverified

- **O3 — Impossible lifecycle states fail as invariant violations with diagnostics instead of being repaired**
  - *Claim:* Reaching a state combination the machine forbids panics with a diagnostic naming the scope and both states, rather than coercing to `Idle`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(lifecycle::impossible_state_is_invariant_violation)'` — expect PASS via `#[should_panic]` with a message containing the scope and the offending transition. `03-runtime-and-turns.md` §Input and queue flow forbids repairing an impossible lifecycle state.
  - *Status:* ☐ unverified

- **O4 — UI projections (`is_processing`, `loop_status`, active run IDs, last stop reason) are derived from `TurnState` and cannot contradict it**
  - *Claim:* Each projection is computed from the current state, with no stored duplicate.
  - *Evidence to collect:* Read `crates/lotta-runtime/src/lifecycle.rs` projection functions and confirm each takes `&TurnState`. Run `cargo nextest run -p lotta-runtime -E 'test(lifecycle::projections_derive_from_state)'` — expect one case per projection.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(lifecycle::)'` and sees lease minting, the four-part stale-lease suppression, the invariant-violation case, and derived projections pass**
  - *Claim:* The lifecycle module passes with all four stale-lease cases present.
  - *Evidence to collect:* Run the filter and confirm zero failures and the four `stale_lease_suppression` cases in the summary.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/registry.rs` (Task 16) holds the runtime handles the lifecycle owner mutates; confirm `registry::eviction` still passes now that residency consults live turn state : ☐ (PRESERVED / REGRESSION)

## Residue

Cancellation semantics for `Cancelling` are Task 57; this task only enforces that a `Cancelling` lease cannot be cleared by a late effect.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
