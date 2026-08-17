# Done Certificate — Task 48: Runtime-owned retry policy and explicit fallback

**Task:** [48-provider_retry_and_fallback.md](48-provider_retry_and_fallback.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-17 — DONE

> Verification protocol for Task 48. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 48) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a deterministic, jitter-free retry policy owned by the runtime, plus explicit transport fallback that never mutates the persisted model.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not add jitter: `06-model-providers.md` §Assumptions *Retry timing* states there is no jitter at the pinned baseline, and `00-overview.md` §Compatibility definition requires retry traces to match.

## Obligations

- **O1 — The retry path contains no jitter and produces a byte-identical delay sequence across runs under a fake clock**
  - *Claim:* Two runs of the same failing sequence produce identical delays, and no randomness source is reachable from the policy.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(retry::deterministic_delays)'` — expect PASS comparing two recorded delay sequences for equality. Grep `crates/lotta-runtime/src/retry/` for `rand`, `random`, and `jitter` — expect zero matches, matching the baseline where jitter appears only in `../letta-code/src/websocket/listen-register.ts` (the Cloud register path), never in the provider-turn path.
  - *Checks:* Resolve any `jitter` symbol in scope — confirm the only one is the schedule's `jitter_offset_ms` (Task 03), which belongs to cron scheduling and must not be reused here.
  - *Status:* ☒ SATISFIED — deterministic selector passes identically across repeated runs; production retry sources contain no random/jitter path.

- **O2 — The policy implements all four delay behaviors: retry-after handling, capped exponential for transient/busy, linear for empty responses, and a total deadline**
  - *Claim:* Each behavior has a test asserting the exact delay sequence it produces.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(retry::delay_shapes)'` — expect four cases. Trace the transient case: attempt 1 → delay d, attempt 2 → 2d, attempt 3 → min(4d, `PROVIDER_BACKOFF_MS_MAX`).
  - *Status:* ☒ SATISFIED — four exact delay-shape cases cover retry-after precedence, capped transient/busy exponential, empty 500/1000 linear delays, and one hard monotonic deadline.

- **O3 — `PROVIDER_RETRIES_MAX` and `PROVIDER_BACKOFF_MS_MAX` carry the spec defaults, and authentication, invalid-request, unsupported-model, and schema errors do not retry**
  - *Claim:* Attempts stop at three, backoff caps at 60,000 ms, and the four non-retryable kinds fail on the first attempt.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(retry::bounds) + test(retry::non_retryable)'` — expect the two bound values from `06-model-providers.md` §Limits and four single-attempt cases.
  - *Status:* ☒ SATISFIED — canonical values are three retries/four transient attempts and 60,000 ms cap; empty responses use the pinned two-retry budget; all four terminal kinds stop once.

- **O4 — Fallback is explicit, emits a retry event naming source and destination transport, and never modifies the persisted model**
  - *Claim:* A configured fallback switches transport, emits the naming event, and leaves the stored model unchanged.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-runtime -E 'test(retry::fallback)'` — expect `emits_named_retry_event` and `does_not_modify_persisted_model`, the second asserting zero store writes.
  - *Status:* ☒ SATISFIED — explicit validated fallback emits lease-guarded FIFO source/destination retry events before zero-delay destination output and leaves connected persisted Agent/Conversation state byte-identical with zero writes.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☒ SATISFIED — workspace 1,553/1,553, fmt, strict all-target/all-feature Clippy, docs, deny, source audit, named constants, and shape checks pass.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(retry::)'` and sees deterministic jitter-free delays, all four delay shapes, both bounds, non-retryable kinds, and model-preserving fallback pass**
  - *Claim:* The retry module passes with determinism asserted.
  - *Evidence to collect:* Run the filter twice and confirm the recorded delay sequences are identical between runs.
  - *Status:* ☒ SATISFIED — exact sibling selectors are all nonzero and broad retry passes 25/25 repeatedly with deterministic traces.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/turn/loop.rs` (Task 19) routes production calls through this policy; confirm `turn::terminal_once` still passes : ☒ PRESERVED — terminal cases pass 9/9, turn cases 28/28, and 256 sequential production retry steps preserve Task19 bounds and terminal-once behavior.

## Residue

Adapters identify retryable errors but do not retry; `06-model-providers.md` §Assumptions *Retry ownership* keeps central policy so nested retries cannot exceed deadlines.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☒ DONE
CONFIDENCE: ☒ high
SUMMARY: Runtime-owned retries now match pinned deterministic timing and budgets, preserve live streaming and one hard deadline, emit ordered lease-guarded retry/fallback events, and cannot mutate persisted models.
