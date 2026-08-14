# Task 01 — Workspace bootstrap, toolchain, and CI gates

**Plan:** [plan.md](../plan.md) · **Certificate:** [01-workspace_bootstrap_toolchain_ci-certificate.md](01-workspace_bootstrap_toolchain_ci-certificate.md)

**Implements:** [architecture-principles.md §Workspace layout](../../../architecture-principles.md#workspace-layout) · [architecture-principles.md §Rust baseline](../../../architecture-principles.md#rust-baseline) · [development-guidelines.md §Toolchain](../../../development-guidelines.md#toolchain) · [development-guidelines.md §Repository hygiene](../../../development-guidelines.md#repository-hygiene)
**Depends on:** —
**Produces:** a Rust workspace with the 13 named library crates plus the root `lotta` composition binary that builds clean and runs every toolchain gate in GitHub Actions
**Pointers:** `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `deny.toml`, `src/main.rs`, `crates/lotta-*/src/lib.rs`, `.github/workflows/ci.yml`

## Steps

- [x] Create the root `lotta` package and workspace `Cargo.toml`, listing exactly the 13 library-crate members named in `architecture-principles.md` §Workspace layout
- [x] Create `src/main.rs` at the workspace root as the composition-root binary, per §Workspace layout's `# composition root only` annotation, with no business logic
- [x] Pin the stable toolchain and edition 2024 in `rust-toolchain.toml`; set `rustfmt.toml` to 100 columns and `clippy.toml` to the pedantic-adjacent lint set
- [x] Add `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` to every crate root, recording the OS-adapter exception path for `lotta-tools`
- [x] Configure `deny.toml` for license, advisory, duplicate, and source policy, and commit `Cargo.lock`
- [x] Add `.github/workflows/ci.yml` running fmt, clippy, nextest, deny, audit, and rustdoc on push and pull request

## Definition of done

- [x] The workspace contains the root `lotta` binary package plus exactly the 13 library crates of `architecture-principles.md` §Workspace layout, with `src/main.rs` as the sole binary target
- [x] The toolchain is pinned to stable with edition 2024, 100-column rustfmt, and a pedantic-adjacent clippy configuration whose every opt-out carries a why comment
- [x] Every crate root carries `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`, with any OS-adapter exception named and justified in the crate doc comment
- [x] The GitHub Actions workflow installs pinned gate tools and runs fmt, clippy with denied warnings, nextest, deny, audit, and rustdoc; every locally available equivalent gate passes before publication
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo build --workspace && cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo nextest run --workspace --all-features && cargo deny check` and sees every command exit 0
