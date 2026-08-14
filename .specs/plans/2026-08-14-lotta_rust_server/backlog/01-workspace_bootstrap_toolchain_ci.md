# Task 01 — Workspace bootstrap, toolchain, and CI gates

**Plan:** [plan.md](../plan.md) · **Certificate:** [01-workspace_bootstrap_toolchain_ci-certificate.md](01-workspace_bootstrap_toolchain_ci-certificate.md)

**Implements:** [architecture-principles.md §Workspace layout](../../../architecture-principles.md#workspace-layout) · [architecture-principles.md §Rust baseline](../../../architecture-principles.md#rust-baseline) · [development-guidelines.md §Toolchain](../../../development-guidelines.md#toolchain) · [development-guidelines.md §Repository hygiene](../../../development-guidelines.md#repository-hygiene)
**Depends on:** —
**Produces:** a 13-crate Rust workspace with a root composition-root binary that builds clean and runs every toolchain gate in GitHub Actions
**Pointers:** `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `deny.toml`, `src/main.rs`, `crates/lotta-*/src/lib.rs`, `.github/workflows/ci.yml`

## Steps

- [ ] Create the workspace `Cargo.toml` listing exactly the 13 member crates named in `architecture-principles.md` §Workspace layout
- [ ] Create `src/main.rs` at the workspace root as the composition-root binary, per §Workspace layout's `# composition root only` annotation, with no business logic
- [ ] Pin the stable toolchain and edition 2024 in `rust-toolchain.toml`; set `rustfmt.toml` to 100 columns and `clippy.toml` to the pedantic-adjacent lint set
- [ ] Add `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` to every crate root, recording the OS-adapter exception path for `lotta-tools`
- [ ] Configure `deny.toml` for license, advisory, duplicate, and source policy, and commit `Cargo.lock`
- [ ] Add `.github/workflows/ci.yml` running fmt, clippy, nextest, deny, audit, and rustdoc on push and pull request

## Definition of done

- [ ] The workspace declares exactly the 13 crates of `architecture-principles.md` §Workspace layout, with `src/main.rs` as the root composition-root binary
- [ ] The toolchain is pinned to stable with edition 2024, 100-column rustfmt, and a pedantic-adjacent clippy configuration whose every opt-out carries a why comment
- [ ] Every crate root carries `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`, with any OS-adapter exception named and justified in the crate doc comment
- [ ] The GitHub Actions workflow runs fmt, clippy with denied warnings, nextest, deny, audit, and rustdoc, and is green on an empty workspace
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo build --workspace && cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo nextest run --workspace --all-features && cargo deny check` and sees every command exit 0
