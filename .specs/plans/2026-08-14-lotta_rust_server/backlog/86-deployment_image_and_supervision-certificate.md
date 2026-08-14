# Done Certificate — Task 86: Deployment image and supervised topology

**Task:** [86-deployment_image_and_supervision.md](86-deployment_image_and_supervision.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 86. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 86) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a non-root container image with the documented mounts, default command, and supervised channel host, alongside the deployment-variant comparison.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not deploy the channel host independently: `07-channels-and-operations.md` §Deployment image states that topology loses the stdin/stdout management plane and is not the initial parity topology.

## Obligations

- **O1 — The image runs as a non-root user with a read-only root filesystem, dropped capabilities, and write access only to state, workspace, and temp**
  - *Claim:* The running container's user is non-root and a write outside the three permitted paths fails.
  - *Evidence to collect:* Build the image, run it, and execute `id -u` (expect non-zero) and a write to `/usr` (expect failure). Run `cargo nextest run --test deployment -E 'test(image::hardening)'` — expect PASS driving these checks.
  - *Status:* ☐ unverified

- **O2 — The default command matches the §Deployment image block, and the four mount points resolve with `LETTA_HOME` and `LETTA_LOCAL_BACKEND_DIR` set correctly**
  - *Claim:* The image's default command and environment match the spec block exactly.
  - *Evidence to collect:* Run `docker inspect` on the built image and compare `Cmd` and `Env` against the §Deployment image block. Run `cargo nextest run --test deployment -E 'test(image::command_and_mounts)'` — expect PASS.
  - *Status:* ☐ unverified

- **O3 — Bun or Node is present only when JS compatibility hosts or channels are enabled, and OS cron only when explicitly configured**
  - *Claim:* The default build contains no JS runtime and no cron daemon; the opt-in build contains them.
  - *Evidence to collect:* Run `cargo nextest run --test deployment -E 'test(image::optional_runtimes)'` — expect a default-build case asserting absence and an opt-in case asserting presence.
  - *Status:* ☐ unverified

- **O4 — The channel host is supervised as a child in the same service, and the deployment-variant comparison is documented**
  - *Claim:* Starting the image with channels enabled produces a supervised child, and `docs/deployment.md` records the six differences from `letta-app-server-deployment`.
  - *Evidence to collect:* Run the image with channels enabled and confirm the child process exists. Read `docs/deployment.md` and confirm it reproduces the six rows of `07-channels-and-operations.md` §Compatibility with letta-app-server-deployment.
  - *Checks:* Resolve the supervision path — confirm it is the Task 78 supervisor inside the same container, not a separate service; §Deployment image states an independently deployed host would lose the stdin/stdout management plane.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `docker build .` then starts the image with a token file mounted and sees the server bind on 4500 as a non-root user, `/readyz` return ready, and the channel host running as a supervised child**
  - *Claim:* The image builds and runs with the documented topology.
  - *Evidence to collect:* Run the build and the run; confirm `curl http://localhost:4500/readyz` returns ready, `docker exec … id -u` is non-zero, and the channel host appears in the container's process list.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `src/main.rs` (Task 85) is the packaged binary; confirm `shutdown::step_order` still passes inside the container : ☐ (PRESERVED / REGRESSION)

## Residue

`07-channels-and-operations.md` §Assumptions leaves whether `letta-app-server-deployment` gains a Lotta profile open; this task ships an image and a documented comparison.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
