# Done Certificate — Task 41: Controller-owned external tools

**Task:** [41-external_tools.md](41-external_tools.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-16 — DONE

> Verification protocol for Task 41. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 41) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** controller-owned tool registration at `runtime_start` and by atomic update, with scoped calls, the fixed five-minute timeout, and typed owner-disconnect rejection.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not let the controller own the call timeout: `05-tools-and-extensions.md` §External tools and MCP states the server owns the fixed five-minute timeout.

## Obligations

- **O1 — Registration accepts unscoped and `scope_id`-selected tools at `runtime_start` and by atomic update, rejecting a whole group atomically on any invalid member**
  - *Claim:* A group with one invalid definition registers none of its members.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(external::registration)'` — expect `registers_unscoped`, `registers_scoped`, and `invalid_member_rejects_group` to pass, the last asserting the registry is unchanged.
  - *Status:* ☒ SATISFIED — registration selector passes 3/3 for runtime-start unscoped/scoped groups and whole-group atomic rejection, with immutable scope selection and optimistic registry publication.

- **O2 — The server owns a fixed five-minute call timeout, and a timed-out call yields a typed timeout result rather than hanging**
  - *Claim:* `EXTERNAL_TOOL_CALL_TIMEOUT_MS` is 300,000 and the call resolves with a timeout outcome at that bound.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(external::timeout)'` — expect PASS with the fake clock advanced to the bound; confirm the constant value against `05-tools-and-extensions.md` §Limits.
  - *Status:* ☒ SATISFIED — timeout selector proves the server-owned 300,000 ms boundary exactly and proves an arbitrary 1 ms raw deadline cannot shorten it.

- **O3 — A pending call is rejected with a typed owner-disconnected result when its originating connection drops, and calls never resolve through another connection**
  - *Claim:* Dropping the owning connection resolves pending calls with the owner-disconnected outcome, and a second connection cannot answer them.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(external::owner_disconnect)'` — expect `pending_rejected_on_disconnect` and `other_connection_cannot_answer` to pass.
  - *Checks:* Resolve the connection used to deliver a call result — confirm it is the registration's originating connection handle, not the most recent subscriber. Answering from another connection would break `05-tools-and-extensions.md` §External tools and MCP.
  - *Status:* ☒ SATISFIED — both named owner-disconnect cases pass; exact owner generation drains its pending calls and another connection cannot settle them.

- **O4 — `EXTERNAL_TOOLS_PER_RUNTIME_MAX` rejects registration at the limit with the 257th tool**
  - *Claim:* The named bound from `01-domain-model.md` §Resource bounds rejects atomically above 256.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(external::bounds)'` — expect below/at/above cases naming `EXTERNAL_TOOLS_PER_RUNTIME_MAX`.
  - *Status:* ☒ SATISFIED — bounds selector covers 255/256/257 across scopes, with the above-limit group and revisions unchanged atomically.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☒ SATISFIED — fmt, strict workspace Clippy, workspace nextest 1,295/1,295, private Rustdoc, and `cargo deny check` pass; touched Rust meets file/function/column limits.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(external::)'` and sees scoped registration, atomic group rejection, the five-minute timeout, owner-disconnect rejection, and the per-runtime cap pass**
  - *Claim:* The external-tool module passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `owner_disconnect` cases.
  - *Status:* ☒ SATISFIED — clean reviewer ran all 17 external cases and found no Task 41 remainder.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/registry.rs` (Task 32) holds the merged registry; confirm `registry::atomic_swap` still passes with external tools present : ☒ PRESERVED — 6/6.

## Residue

The WebSocket commands that carry registration and call responses are Task 62.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☒ DONE
CONFIDENCE: ☒ high
SUMMARY: Runtime-scoped controller tools register atomically with immutable scope selection, execute through owner-bound correlated calls, enforce the fixed server-owned five-minute timeout and 256-tool cap, and reject disconnects or cross-owner responses with typed outcomes; all repository gates pass.
