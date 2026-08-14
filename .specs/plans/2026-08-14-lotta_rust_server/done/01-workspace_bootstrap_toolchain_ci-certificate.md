# Done Certificate — Task 01: Workspace bootstrap, toolchain, and CI gates

**Task:** [01-workspace_bootstrap_toolchain_ci.md](01-workspace_bootstrap_toolchain_ci.md) · **Plan:** [plan.md](../plan.md)
**State:** Validated 2026-08-14

> Verification protocol for Task 01. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 01) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a Rust workspace with the 13 named library crates plus the root `lotta` composition binary that builds clean and runs every toolchain gate in GitHub Actions.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Nothing exists before this task; the constraint is that the crate set and binary location must not diverge from `architecture-principles.md` §Workspace layout, because every later task's `Pointers` resolve against it.

## Obligations

- **O1 — The workspace contains the root `lotta` binary package plus exactly the 13 library crates of `architecture-principles.md` §Workspace layout, with `src/main.rs` as the sole binary target**
  - *Claim:* `cargo metadata` lists the root `lotta` package and 13 library packages whose names match the §Workspace layout list, and the only binary target is the workspace-root `src/main.rs`.
  - *Evidence to collect:* Run `cargo metadata --no-deps --format-version 1`; expect 14 workspace packages total: root package `lotta` plus the 13 named libraries (`lotta-domain`, `lotta-protocol`, `lotta-runtime`, `lotta-store`, `lotta-memfs`, `lotta-providers`, `lotta-tools`, `lotta-extensions`, `lotta-channels`, `lotta-app-server`, `lotta-config`, `lotta-telemetry`, `lotta-testkit`). Inspect every target in the metadata; expect one binary target named `lotta` at `src/main.rs` and 13 library targets under `crates/*/src/lib.rs`, with no crate-local binary target.
  - *Checks:* Resolve the `main` symbol the binary target compiles: confirm it is `src/main.rs::main`, not `crates/lotta-app-server/src/main.rs`. Flag `NAME SHADOWING` if a second `main.rs` exists anywhere under `crates/`.
  - *Evidence:* `cargo metadata --no-deps --format-version 1` exited 0 and reported 14 workspace packages: root `lotta` plus the exact 13 named libraries. Its 14 targets are exactly 13 `lib` targets at `crates/*/src/lib.rs` and one `bin` target `lotta` at workspace-root `src/main.rs`. A recursive check under `crates/` found zero `main.rs` files, so `src/main.rs::main` is the sole resolved binary entry point and there is no name shadowing.
  - *Status:* SATISFIED

- **O2 — The toolchain is pinned to stable with edition 2024, 100-column rustfmt, and a pedantic-adjacent clippy configuration whose every opt-out carries a why comment**
  - *Claim:* `rust-toolchain.toml` pins a stable channel; `rustfmt.toml` sets `max_width = 100`; `clippy.toml` opt-outs each carry a comment.
  - *Evidence to collect:* Read `rust-toolchain.toml` and confirm `channel` is a pinned stable version and `edition = "2024"` appears in `Cargo.toml`'s workspace package table. Read `rustfmt.toml` for `max_width = 100`. Read `clippy.toml` and confirm every `allow` entry is preceded by a `#` comment giving the reason.
  - *Evidence:* `rust-toolchain.toml:2` pins stable Rust `1.96.1`; `Cargo.toml:28` sets workspace edition `2024`; `rustfmt.toml:2` sets `max_width = 100`. `Cargo.toml:36-38` denies Clippy `all` and `pedantic`; `clippy.toml:1-2` documents that central policy and contains no opt-outs requiring justification.
  - *Status:* SATISFIED

- **O3 — Every crate root carries `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`, with any OS-adapter exception named and justified in the crate doc comment**
  - *Claim:* All 13 `lib.rs` files (and `src/main.rs`) carry both attributes, or carry a documented exception.
  - *Evidence to collect:* Grep `crates/*/src/lib.rs` and `src/main.rs` for `forbid(unsafe_code)` and `deny(missing_docs)` — expect one match each per file. For any file lacking `forbid(unsafe_code)`, read its crate doc comment and confirm it names the OS adapter and the review owner required by `architecture-principles.md` §Unsafe code.
  - *Evidence:* Enumerated the 14 crate roots (`src/main.rs` plus 13 `crates/*/src/lib.rs` files); every file contains exactly one `#![forbid(unsafe_code)]` and exactly one `#![deny(missing_docs)]`. `crates/lotta-tools/src/lib.rs:3-6` names and justifies the future isolated OS-sandbox exception path while explicitly stating no exception is enabled now.
  - *Status:* SATISFIED

- **O4 — The GitHub Actions workflow installs pinned gate tools and runs fmt, clippy with denied warnings, nextest, deny, audit, and rustdoc; every locally available equivalent gate passes before publication**
  - *Claim:* `.github/workflows/ci.yml` installs pinned external gate tools and invokes all six gates with the commands from `development-guidelines.md` §Toolchain; every equivalent available in the review workspace passes before the first push.
  - *Evidence to collect:* Read `.github/workflows/ci.yml`; confirm pinned installations for `cargo-nextest`, `cargo-deny`, and `cargo-audit`, and exact invocations of `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, `cargo deny check`, `cargo audit`, and `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS=-D warnings`. In the task workspace run each locally installed equivalent and record exit code 0; if `cargo-audit` is absent locally, record that fact and verify its pinned CI installation rather than treating a pre-publication remote run as available evidence.
  - *Evidence:* `.github/workflows/ci.yml:20` enumerates fmt, clippy, nextest, deny, audit, and rustdoc. Lines 27-30 install pinned `cargo-nextest@0.9.126`, `cargo-deny@0.19.0`, and `cargo-audit@0.22.0`; lines 31-50 invoke every required command, including `-D warnings` for Clippy and `RUSTDOCFLAGS: -D warnings` for rustdoc. Workflow YAML parsed successfully. Independent local runs all exited 0 for build, fmt, clippy, nextest (1/1 passed), deny (advisories/bans/licenses/sources all OK), and rustdoc. Local `cargo audit --version` returned “no such command”; under the certificate's explicit pre-publication rule this is acceptable because CI installs the pinned audit tool and runs `cargo audit`.
  - *Status:* SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Evidence:* Independent runs of fmt, Clippy, nextest, and deny exited 0. The task introduces no constants or bounds. `src/main.rs::main` and its test are each one line, every Rust source line is within 100 columns, and no task-added function approaches the 70-line limit.
  - *Status:* SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo build --workspace && cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo nextest run --workspace --all-features && cargo deny check` and sees every command exit 0**
  - *Claim:* The five-command chain completes with exit code 0 and no warning output on a clean checkout.
  - *Evidence to collect:* On a clean clone, run the five-command chain and record each exit code — expect 0 from all five and zero lines matching `^warning:` from the build and clippy steps.
  - *Evidence:* Ran the exact five-command chain in the isolated task workspace; build, fmt, clippy, nextest, and deny each exited 0. Captured build and Clippy output contained zero lines beginning `warning:`.
  - *Status:* SATISFIED

## Regression check

No existing callers in scope — confirmed from the complete `jj diff --from @- --to @`: every implementation file is newly added, the task has no dependencies, and no pre-existing unit or caller is modified. **PRESERVED.**

## Residue

The OS-sandbox adapter exception to `#![forbid(unsafe_code)]` is declared here but has no code to guard until Task 35. `architecture-principles.md` §Assumptions records workspace granularity as open; this task creates all 13 crates because the layout is the anchor every later `Pointers` line resolves against.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: DONE
CONFIDENCE: high
SUMMARY: O1–O6 are all SATISFIED with direct file, metadata, and execution evidence; the greenfield regression surface is PRESERVED, and local cargo-audit absence is covered by the certificate's pinned-CI pre-publication rule.
