# Done Certificate — Task 09: Testkit fakes and the shared port-contract suite

**Task:** [09-testkit_and_port_contract_suite.md](09-testkit_and_port_contract_suite.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 09. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 09) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** deterministic clocks and IDs, in-memory port fakes, and one shared contract suite that every concrete adapter and its fake must pass.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not let the contract suite diverge per adapter: `architecture-principles.md` §Testing architecture requires every concrete adapter to pass the same suite as its in-memory fake.

## Obligations

- **O1 — One shared contract suite exists per port and is invoked generically, so a concrete adapter reuses it without copying assertions**
  - *Claim:* `lotta_testkit::contract` exposes a per-port suite function taking an implementation, and the fakes call it.
  - *Evidence to collect:* Read `crates/lotta-testkit/src/contract/mod.rs` and confirm each suite is a generic function (for example `pub fn agent_store_contract<S: AgentStore>(store: S)`), not a set of fake-specific tests. Run `cargo nextest run -p lotta-testkit -E 'test(contract::)'` — expect one passing suite invocation per port.
  - *Status:* ☐ unverified

- **O2 — Every port from Tasks 06–08 has an in-memory fake that passes the shared suite**
  - *Claim:* The fake set covers all ports and no port is missing a fake.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-testkit -E 'test(fakes::)'` — expect one passing case per port. Read the fakes module list and compare it against `crates/lotta-runtime/src/ports/mod.rs` exports — expect a fake for each trait.
  - *Checks:* Resolve the `ProviderPort` implementation the fake provides — confirm it satisfies the Task 07 trait including all ten `ProviderEvent` variants, not a reduced subset.
  - *Status:* ☐ unverified

- **O3 — `FakeClock` and the deterministic ID generators make tests reproducible: no wall-clock read, no sleep, no network**
  - *Claim:* Running the same test twice yields byte-identical timestamps and IDs, and no forbidden call is reachable.
  - *Evidence to collect:* Grep `crates/lotta-testkit/src/` for `thread::sleep`, `tokio::time::sleep`, `SystemTime::now`, `Utc::now`, and `TcpStream::connect` — expect zero matches outside a documented local-fake-server helper. Run `cargo nextest run -p lotta-testkit -E 'test(clock::) + test(ids::)'` twice and diff the emitted timestamps — expect identical output.
  - *Status:* ☐ unverified

- **O4 — The golden-fixture loader reads `fixtures/` deterministically and fails loudly on a missing or malformed fixture**
  - *Claim:* Loading an absent fixture returns a typed error naming the path, and loading a malformed one fails rather than silently returning an empty corpus.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-testkit -E 'test(fixtures::)'` — expect `missing_fixture_is_error` and `malformed_fixture_is_error` to pass. Trace: `load("fixtures/protocol/absent.json")` → error naming `fixtures/protocol/absent.json`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-testkit` and sees the shared contract suite pass against every in-memory fake with no wall-clock or network dependency**
  - *Claim:* The whole testkit crate's tests pass, including one contract-suite invocation per port.
  - *Evidence to collect:* Run the command and confirm zero failures; then run it with the network disabled and confirm the same result.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/ports/` trait signatures are consumed here; confirm Task 07's `ports::streaming_invariants` still passes when driven through the fake provider : ☐ (PRESERVED / REGRESSION)

## Residue

Adapters do not exist yet, so the suite currently proves only fake-vs-contract. Tasks 22, 29, 49–51, and 37–40 each add a real implementation that must call the same suite.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
