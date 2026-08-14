# Done Certificate — Task 01: Workspace bootstrap, toolchain, and CI gates

**Task:** [01-workspace_bootstrap_toolchain_ci.md](01-workspace_bootstrap_toolchain_ci.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 01. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 01) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a 13-crate Rust workspace with a root composition-root binary that builds clean and runs every toolchain gate in GitHub Actions.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Nothing exists before this task; the constraint is that the crate set and binary location must not diverge from `architecture-principles.md` §Workspace layout, because every later task's `Pointers` resolve against it.

## Obligations

- **O1 — The workspace declares exactly the 13 crates of `architecture-principles.md` §Workspace layout, with `src/main.rs` as the root composition-root binary**
  - *Claim:* `cargo metadata` lists 13 workspace members whose names match the §Workspace layout list, and the only binary target is the workspace-root `src/main.rs`.
  - *Evidence to collect:* Run `cargo metadata --no-deps --format-version 1` and compare the member `name` set against the 13 names in `architecture-principles.md` §Workspace layout (`lotta-domain`, `lotta-protocol`, `lotta-runtime`, `lotta-store`, `lotta-memfs`, `lotta-providers`, `lotta-tools`, `lotta-extensions`, `lotta-channels`, `lotta-app-server`, `lotta-config`, `lotta-telemetry`, `lotta-testkit`) — expect set equality. Read `Cargo.toml` and confirm the `[[bin]]`/default binary path is `src/main.rs`, not a crate-local `main.rs`.
  - *Checks:* Resolve the `main` symbol the binary target compiles: confirm it is `src/main.rs::main`, not `crates/lotta-app-server/src/main.rs`. Flag `NAME SHADOWING` if a second `main.rs` exists anywhere under `crates/`.
  - *Status:* ☐ unverified

- **O2 — The toolchain is pinned to stable with edition 2024, 100-column rustfmt, and a pedantic-adjacent clippy configuration whose every opt-out carries a why comment**
  - *Claim:* `rust-toolchain.toml` pins a stable channel; `rustfmt.toml` sets `max_width = 100`; `clippy.toml` opt-outs each carry a comment.
  - *Evidence to collect:* Read `rust-toolchain.toml` and confirm `channel` is a pinned stable version and `edition = "2024"` appears in `Cargo.toml`'s workspace package table. Read `rustfmt.toml` for `max_width = 100`. Read `clippy.toml` and confirm every `allow` entry is preceded by a `#` comment giving the reason.
  - *Status:* ☐ unverified

- **O3 — Every crate root carries `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`, with any OS-adapter exception named and justified in the crate doc comment**
  - *Claim:* All 13 `lib.rs` files (and `src/main.rs`) carry both attributes, or carry a documented exception.
  - *Evidence to collect:* Grep `crates/*/src/lib.rs` and `src/main.rs` for `forbid(unsafe_code)` and `deny(missing_docs)` — expect one match each per file. For any file lacking `forbid(unsafe_code)`, read its crate doc comment and confirm it names the OS adapter and the review owner required by `architecture-principles.md` §Unsafe code.
  - *Status:* ☐ unverified

- **O4 — The GitHub Actions workflow runs fmt, clippy with denied warnings, nextest, deny, audit, and rustdoc, and is green on an empty workspace**
  - *Claim:* `.github/workflows/ci.yml` invokes all six gates with the exact commands from `development-guidelines.md` §Toolchain, and the workflow passes.
  - *Evidence to collect:* Read `.github/workflows/ci.yml` and confirm it runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, `cargo deny check`, `cargo audit`, and `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS=-D warnings`. Run `gh run list --workflow ci.yml --limit 1` — expect the newest run's conclusion to be `success`.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo build --workspace && cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo nextest run --workspace --all-features && cargo deny check` and sees every command exit 0**
  - *Claim:* The five-command chain completes with exit code 0 and no warning output on a clean checkout.
  - *Evidence to collect:* On a clean clone, run the five-command chain and record each exit code — expect 0 from all five and zero lines matching `^warning:` from the build and clippy steps.
  - *Status:* ☐ unverified

## Regression check

No existing callers in scope — this task introduces new units that nothing else calls yet, and modifies no unit produced by a task it depends on.

## Residue

The OS-sandbox adapter exception to `#![forbid(unsafe_code)]` is declared here but has no code to guard until Task 35. `architecture-principles.md` §Assumptions records workspace granularity as open; this task creates all 13 crates because the layout is the anchor every later `Pointers` line resolves against.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
