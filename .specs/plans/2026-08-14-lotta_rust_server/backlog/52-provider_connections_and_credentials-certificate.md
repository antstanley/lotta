# Done Certificate — Task 52: Provider connections, `auth.json` v1, and forced disconnect

**Task:** [52-provider_connections_and_credentials.md](52-provider_connections_and_credentials.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 52. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 52) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** baseline-compatible plaintext `providers/auth.json` v1 with restrictive modes, redaction, and disconnect that refuses while turns are active unless forced.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not encrypt the live `auth.json`: `06-model-providers.md` §Assumptions *Credential compatibility* keeps it plaintext with restrictive permissions so TypeScript round trips still work.

## Obligations

- **O1 — `providers/auth.json` v1 round-trips against the fixture with configuration and secrets together in plaintext, at modes `0700`/`0600`**
  - *Claim:* The written file matches the version-1 record map shape and carries the two permission modes.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(connections::auth_json)'` — expect `round_trips_fixture` and `enforces_modes`, the second reading the on-disk permission bits.
  - *Checks:* Resolve the encryption path for the live file — confirm there is none. `04-persistence-and-memfs.md` §Local-backend directory layout states the TypeScript runtime cannot read a `credentials.enc` replacement, so live at-rest encryption would break round trips.
  - *Status:* ☐ unverified

- **O2 — Secrets reach adapters but never diagnostics: no captured log, error, or App Server snapshot contains a credential**
  - *Claim:* A connect, a failed connect, and a snapshot all omit the secret value.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(connections::redaction)'` — expect three cases asserting the marker secret is absent from captured tracing output, the error rendering, and the snapshot JSON.
  - *Status:* ☐ unverified

- **O3 — Disconnect refuses while active turns use the connection unless `force` is explicit, and a forced disconnect cancels affected turns first**
  - *Claim:* An unforced disconnect during an active turn errors; a forced one cancels the turn before removing the connection.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(connections::disconnect)'` — expect `refuses_while_active` and `force_cancels_then_disconnects`, the second asserting the cancellation precedes removal in the recorded order.
  - *Status:* ☐ unverified

- **O4 — `PROVIDERS_MAX` bounds provider connections and is distinct from the WebSocket `CONNECTIONS_MAX`**
  - *Claim:* The 129th provider connection is rejected, and the constant used is not the 1,024 WebSocket cap.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(connections::bounds)'` — expect below/at/above cases at 128.
  - *Checks:* Resolve the cap constant at the connection-registration site — confirm it is `PROVIDERS_MAX` (`06-model-providers.md` §Limits, 128), not `CONNECTIONS_MAX` (`01-domain-model.md` §Resource bounds, 1,024). `NAME SHADOWING`: both are `…_MAX` connection caps with different meanings.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(connections::)'` and sees the plaintext v1 round trip at `0600`, three redaction cases, forced-disconnect ordering, and the 128 provider cap pass**
  - *Claim:* The connections module passes against the fixture.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `enforces_modes` case reading real permission bits.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/atomic.rs` (Task 22) writes this file; confirm `atomic::modes` still passes for `providers/` : ☐ (PRESERVED / REGRESSION)

## Residue

`04-persistence-and-memfs.md` and `06-model-providers.md` both record live credential encryption as an open question; backups are encrypted with an operator-supplied key in Task 87.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
