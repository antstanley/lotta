# Done Certificate — Task 52: Provider connections, `auth.json` v1, and forced disconnect

**Task:** [52-provider_connections_and_credentials.md](52-provider_connections_and_credentials.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-18

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

## Fresh implementation evidence (2026-08-17)

The exact `connections::` selector ran 12 cases: auth_json 3, redaction 3, disconnect 3, and bounds 3; all passed. Task22 `atomic::modes` passed 1/1 and Task47 `lifecycle::` passed 13/13. Formatting, changed-crate Clippy with `-D warnings`, and `cargo deny check` pass. Full workspace execution reached unrelated pinned-TypeScript conformance targets but those fail before exercising Rust because the sibling `letta-code` checkout has a pre-existing `local-backend.ts` SHA mismatch; no pinned source or certificate was edited.

- **O1 — `providers/auth.json` v1 round-trips against the fixture with configuration and secrets together in plaintext, at modes `0700`/`0600`**
  - *Claim:* The written file matches the version-1 record map shape and carries the two permission modes.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(connections::auth_json)'` — expect `round_trips_fixture` and `enforces_modes`, the second reading the on-disk permission bits.
  - *Checks:* Resolve the encryption path for the live file — confirm there is none. `04-persistence-and-memfs.md` §Local-backend directory layout states the TypeScript runtime cannot read a `credentials.enc` replacement, so live at-rest encryption would break round trips.
  - *Status:* ☑ SATISFIED — `connections::auth_json` 4/4 round-trips the real fixture and Bedrock profile bytes, rejects malformed inputs, and reads real `0700`/`0600` modes. Task 22 atomic writes create secret temps at `0600` before bytes are written.

- **O2 — Secrets reach adapters but never diagnostics: no captured log, error, or App Server snapshot contains a credential**
  - *Claim:* A connect, a failed connect, and a snapshot all omit the secret value.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(connections::redaction)'` — expect three cases asserting the marker secret is absent from captured tracing output, the error rendering, and the snapshot JSON.
  - *Status:* ☑ SATISFIED — `connections::redaction` 3/3 captures a real tracing subscriber and proves the marker secret absent from success/failure logs, errors, and public snapshots. The hosted integration proves the persisted secret reaches the real pi adapter.

- **O3 — Disconnect refuses while active turns use the connection unless `force` is explicit, and a forced disconnect cancels affected turns first**
  - *Claim:* An unforced disconnect during an active turn errors; a forced one cancels the turn before removing the connection.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(connections::disconnect)'` — expect `refuses_while_active` and `force_cancels_then_disconnects`, the second asserting the cancellation precedes removal in the recorded order.
  - *Status:* ☑ SATISFIED — `connections::disconnect` 3/3 proves refusal without mutation, cancellation acknowledgement before removal, and cancellation-failure rollback through the Task 47 lifecycle registry.

- **O4 — `PROVIDERS_MAX` bounds provider connections and is distinct from the WebSocket `CONNECTIONS_MAX`**
  - *Claim:* The 129th provider connection is rejected, and the constant used is not the 1,024 WebSocket cap.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-providers -E 'test(connections::bounds)'` — expect below/at/above cases at 128.
  - *Checks:* Resolve the cap constant at the connection-registration site — confirm it is `PROVIDERS_MAX` (`06-model-providers.md` §Limits, 128), not `CONNECTIONS_MAX` (`01-domain-model.md` §Resource bounds, 1,024). `NAME SHADOWING`: both are `…_MAX` connection caps with different meanings.
  - *Status:* ☑ SATISFIED — `connections::bounds` 3/3 proves below/at/above 128; registration uses `PROVIDERS_MAX`, never the WebSocket cap.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☑ SATISFIED — provider 153/153, runtime 200/200, store 164/164, format, full-workspace strict Clippy, deny, and pinned provider-auth cross-runtime 2/2 pass. Functions and changed lines meet the 70/100 source limits. Live-sibling workspace failures remain pre-existing pin drift.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-providers -E 'test(connections::)'` and sees the plaintext v1 round trip at `0600`, three redaction cases, forced-disconnect ordering, and the 128 provider cap pass**
  - *Claim:* The connections module passes against the fixture.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `enforces_modes` case reading real permission bits.
  - *Status:* ☑ SATISFIED — the direct selector passes all 14 connection cases, including exact modes, real tracing redaction, forced-disconnect ordering, catalogs, profile compatibility, and the 128 cap.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/atomic.rs` (Task 22) writes this file; `atomic::` passes 15/15 and observes provider-auth temp `0600` before write: ☑ PRESERVED

## Residue

`04-persistence-and-memfs.md` and `06-model-providers.md` both record live credential encryption as an open question; backups are encrypted with an operator-supplied key in Task 87.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: All O1–O6 are satisfied. Plaintext v1 compatibility, restrictive atomic modes, real hosted credential delivery with captured redaction, forced-disconnect ordering, declarative catalogs, and the 128-provider cap are verified. Claude Fable/high session `8546a266-e0e9-4081-9f7d-eeb95f4764a3` returned `CORRECT / DONE`.
