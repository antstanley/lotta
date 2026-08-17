# Done Certificate — Task 45: Mod compatibility host and capability scoping

**Task:** [45-mod_compatibility_host.md](45-mod_compatibility_host.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-16 — DONE

> Verification protocol for Task 45. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 45) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a versioned JSON-RPC mod host over the shared sidecar framing, with declared-capability scoping, reload, dispose, generation invalidation, diagnostics, safe mode, and `--no-mods`.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not give the host direct access to Rust memory: `05-tools-and-extensions.md` §Hooks and mods states it receives only declared capabilities and a scoped conversation handle.

## Obligations

- **O1 — Mods register all six registration kinds through the host, and every registration is attributed to its owning mod**
  - *Claim:* Tools, commands, providers, permissions, lifecycle events, and UI metadata each register and carry their owner.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(mods::registrations)'` — expect six cases, each asserting the owner field on the resulting registration.
  - *Status:* ☒ SATISFIED — six host-driven registration cases prove owner and generation attribution; real Bun TypeScript registration and genuine tool-call correlation pass, with callable command and lifecycle adapters.

- **O2 — The host receives only declared capabilities and a scoped conversation handle; an undeclared capability call is refused**
  - *Claim:* A mod calling a capability it did not declare receives a refusal and the host cannot reach another conversation.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(mods::capabilities)'` — expect `undeclared_capability_refused` and `handle_is_scoped_to_one_conversation` to pass.
  - *Checks:* Resolve the conversation handle the host receives — confirm it is bound to the runtime scope that loaded the mod, not a registry-wide accessor.
  - *Status:* ☒ SATISFIED — declared capability calls traverse the real framed actor and private broker scope, while undeclared, forged-handle, cross-conversation, and stale-generation requests are refused before the capability port.

- **O3 — Reload, dispose, generation invalidation, diagnostics, and safe mode all work, and a stale generation's registrations are invalidated**
  - *Claim:* Each lifecycle operation has an observable effect and a post-reload stale registration is not callable.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(mods::lifecycle)'` — expect five cases plus `stale_generation_registration_is_invalid`.
  - *Status:* ☒ SATISFIED — owner-selective multi-host load/reload/dispose, generation invalidation, bounded owner diagnostics, SafeMode filtering, failed-candidate preservation, and exact child cleanup pass through the production controller.

- **O4 — `--no-mods` starts without the host and removes every mod-owned registration**
  - *Claim:* With `--no-mods`, no host process is spawned and the registry contains zero mod-owned entries.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(mods::no_mods_flag)'` — expect PASS asserting zero spawned children and zero mod-owned registrations.
  - *Status:* ☒ SATISFIED — controller NoMods startup performs zero incoming spawns, atomically removes every mod owner across all six kinds and Task32, disposes active hosts, and preserves native/external/MCP registrations, toolset, and allowlist.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☒ SATISFIED — all 1,458 workspace tests, all-target build/tests, fmt, strict all-target/all-feature Clippy, private Rustdoc, deny, pinned bridge hash, source shape, duplicate-codec, and orphan-process gates pass.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(mods::)'` and sees six registration kinds, capability refusal, the five lifecycle operations, and `--no-mods` pass**
  - *Claim:* The mod host module passes over the shared framing.
  - *Evidence to collect:* Run the filter and confirm zero failures, then grep `crates/lotta-extensions/src/mods/` for a private framing implementation — expect none.
  - *Status:* ☒ SATISFIED — clean reviewers ran 69/69 substantive mod-host cases and found no remaining framing, capability, transaction, generation, lifecycle, cancellation, process-identity, safe-mode, no-mods, source-shape, or leak defect.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-extensions/src/sidecar/` (Task 42) frames these messages; confirm `sidecar::supervisor` still passes with the mod host attached : ☒ PRESERVED — 3/3 supervisor cases and the shared framing suite pass with the real host actor.
- `crates/lotta-tools/src/registry.rs` (Task 32) receives mod-owned tools; confirm `registry::atomic_swap` still passes : ☒ PRESERVED — Task32 registry cases and mod transaction old/new/failure proofs pass with complete executor, toolset, and allowlist preservation.

## Residue

`05-tools-and-extensions.md` §Assumptions leaves which UI panel/statusline capabilities matter to a headless server open; the host accepts the registrations and the transport ignores unusable ones.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☒ DONE
CONFIDENCE: ☒ high
SUMMARY: Existing TypeScript mods now run in a real isolated Bun/Node compatibility host over Task 42 framing, with private declared capabilities, atomic six-kind generation publication, bounded cancellation-safe RPC, deterministic child lifecycle, SafeMode, diagnostics, and NoMods removal.
