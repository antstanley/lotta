# Done Certificate — Task 83: Configuration and CLI surface

**Task:** [83-configuration_and_cli_surface.md](83-configuration_and_cli_surface.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 83. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 83) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the `lotta server` and `lotta local-backend` CLI surface with validated configuration and secret references, and no configuration format beyond what the spec authorizes.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not invent a configuration file format or environment prefix: no canonical page defines one, and `development-guidelines.md` §Guidelines for AI agents requires updating the spec before adding a surface.

## Obligations

- **O1 — The CLI exposes exactly the flags `02-app-server-api.md` §Listener configuration names, plus `--openai-api` and `--no-mods`, and rejects an unknown flag**
  - *Claim:* Each documented flag parses and an undocumented flag is an error.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-config -E 'test(cli::flags)'` — expect one case per documented flag plus `rejects_unknown_flag`. Compare against the §Listener configuration usage block.
  - *Status:* ☐ unverified

- **O2 — `LETTA_LOCAL_BACKEND_DIR` and `LETTA_HOME` resolve with the documented defaults and overrides**
  - *Claim:* Unset variables yield `~/.letta/lc-local-backend` and `~/.letta`; set variables override them.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-config -E 'test(cli::env_resolution)'` — expect four cases (default and override for each).
  - *Status:* ☐ unverified

- **O3 — Secret material is supplied only by absolute file reference, resolved once at startup into a `Secret`, with no inline secret flag**
  - *Claim:* There is no flag accepting an inline token or shared secret, and the file contents never leave the `Secret` wrapper.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-config -E 'test(cli::secret_refs)'` — expect `requires_absolute_path`, `reads_once_at_startup`, and `no_inline_secret_flag`, the last asserting the parser rejects an inline value.
  - *Status:* ☐ unverified

- **O4 — Configuration is validated before the listener binds, and a failure names the offending flag**
  - *Claim:* An invalid value fails startup with a message naming the flag and the socket is never opened.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-config -E 'test(cli::validate_before_bind)'` — expect PASS asserting the error message contains the flag name and that no listener was created.
  - *Checks:* Resolve the validation call's position relative to the bind call in `src/main.rs` — confirm validation precedes bind; `02-app-server-api.md` §Listener configuration requires non-loopback-without-auth to fail before listening.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `lotta server --help`, then `lotta server --backend local --listen --ws-auth capability-token` without a token file, and sees the documented flag list and a startup failure naming the missing flag before any socket opens**
  - *Claim:* The CLI surface and validation ordering are observable.
  - *Evidence to collect:* Run both commands; confirm the help text matches the §Listener configuration block and that the failure message names `--ws-token-file` or `--ws-token-sha256` with no listener bound.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-app-server/src/listener.rs` (Task 14) consumes this configuration; confirm `listener::url_resolution` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

A general configuration-file format is deliberately out of scope; if one is wanted, a change spec must define it first. This is recorded in `plan.md` §Open questions.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
