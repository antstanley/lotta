# Done Certificate — Task 34: Permission modes, file-backed scopes, and shell analysis

**Task:** [34-permissions_model.md](34-permissions_model.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 34. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 34) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the four permission modes with the `unrestricted` default, three file-backed scopes plus the legacy XDG path, and shell-analysis bypass rejection.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not default to a stricter mode than the baseline: `05-tools-and-extensions.md` §Permissions and sandbox pins the default to `unrestricted`, and changing it would alter observable client behavior without a change spec.

## Obligations

- **O1 — All four permission modes exist with `unrestricted` as the default, and each mode's decision differs where the baseline differs**
  - *Claim:* The mode enum has four variants, `Default` yields `unrestricted`, and an edit action resolves differently under `standard`, `acceptEdits`, and `strict`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(permissions::modes)'` — expect `four_modes`, `default_is_unrestricted`, and three per-mode decision cases. Compare mode names against `../letta-code/src/permissions/mode.ts`.
  - *Checks:* Resolve the default construction — confirm it returns `unrestricted`, matching `05-tools-and-extensions.md` §Permissions and sandbox (`the pinned default is unrestricted`), not a safer-looking `standard`.
  - *Status:* ☐ unverified

- **O2 — Rules load from the three file-backed scopes plus the legacy XDG path, with session and mod rules layered at check time and no agent-file scope**
  - *Claim:* All four file locations contribute rules in the documented precedence, and there is no agent-file loader.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(permissions::scopes)'` — expect one case per location plus `session_and_mod_layer_at_check_time`. Grep the crate for an agent-file permission loader — expect zero, per §Permissions and sandbox (`There is no baseline agent-file permission scope`).
  - *Status:* ☐ unverified

- **O3 — Matching operates on normalized tool name, command, path, cwd, runtime scope, and requested action, with paths canonicalized before policy**
  - *Claim:* A rule matches on each of the six inputs, and a non-canonical path is canonicalized before the matcher sees it.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(permissions::matching)'` — expect six input cases plus `canonicalizes_before_policy`. Trace: `./a/../b/file` → canonicalized `<cwd>/b/file` → matched.
  - *Checks:* Resolve the canonicalization call — confirm it runs before the matcher, not after. A post-match canonicalization would let `..` bypass a path rule.
  - *Status:* ☐ unverified

- **O4 — Shell analysis rejects bypasses rather than relying on string prefixes, with negative tests for symlink traversal, `..`, alternate separators, redirection, subprocess launch, and command substitution**
  - *Claim:* Each of the six bypass classes is rejected, and the rejection comes from the analyzer rather than a prefix check.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(permissions::analyzer)'` — expect six rejection cases. Read one and confirm it exercises a command whose prefix is allowed but whose analyzed effect is not (for example `cat allowed.txt; cat /etc/shadow`).
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(permissions::)'` and sees four modes with the `unrestricted` default, four scope loaders, six matching inputs, and six bypass rejections pass**
  - *Claim:* The permissions module passes with the analyzer cases present.
  - *Evidence to collect:* Run the filter and confirm zero failures and six `analyzer` cases.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-tools/src/pipeline.rs` (Task 33) gains a real permission gate in place of the stub; confirm `pipeline::stage_order` still passes and the gate occupies the same position : ☐ (PRESERVED / REGRESSION)

## Residue

OS sandbox enforcement is Task 35; this task decides policy and returns allow/deny/ask.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
