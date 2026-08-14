# Done Certificate — Task 32: Tool registry, toolset resolution, and atomic swap

**Task:** [32-tool_registry_and_toolsets.md](32-tool_registry_and_toolsets.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

> Verification protocol for Task 32. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 32) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** the six toolset IDs with per-toolset model-facing names, `auto` resolution, allowlist filtering, and side-built registries swapped atomically.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not add or drop a toolset ID: `05-tools-and-extensions.md` §Tool registry states the IDs are exactly those six and the baseline confirms `codex_snake`/`gemini_snake`.

## Obligations

- **O1 — The toolset ID set is exactly the six spec IDs, `auto` resolves to one of them, and per-toolset model-facing names match the baseline**
  - *Claim:* `ToolsetId` has six variants; `auto` is not a member but resolves to one; the name maps match `../letta-code/src/tools/toolset.ts`.
  - *Evidence to collect:* Read `crates/lotta-tools/src/toolset.rs` and count variants — expect six. Run `cargo nextest run -p lotta-tools -E 'test(toolset::)'` — expect `ids_are_exactly_six`, `auto_resolves`, and `model_facing_names_match_baseline`, the last comparing against the extracted name table. Confirm `codex_snake` and `gemini_snake` appear, as at `../letta-code/src/tools/toolset.ts:198,204`.
  - *Checks:* Resolve `auto` in the toolset type — confirm it is a preference value handled before resolution, not a seventh `ToolsetId` variant; §Tool registry lists exactly six IDs.
  - *Status:* ☐ unverified

- **O2 — Internal `Task` is exposed globally as `Agent`, and every alias calls one internal implementation**
  - *Claim:* The model-facing name for the internal `Task` tool is `Agent` in every toolset, and aliases share an implementation.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(names::task_is_agent) + test(names::aliases_share_impl)'` — expect both PASS; the second asserts pointer or ID equality of the resolved executor across aliases.
  - *Status:* ☐ unverified

- **O3 — An allowlist filters both built-ins and external tools, and an empty allowlist exposes none**
  - *Claim:* With an allowlist of one built-in, only that tool is exposed; with an empty allowlist, zero tools are exposed.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(allowlist::)'` — expect `filters_builtins`, `filters_external`, and `empty_exposes_none` to pass.
  - *Status:* ☐ unverified

- **O4 — Registry updates are built aside, validated, then swapped atomically; a failed update leaves the previous registry intact and `TOOLS_LOADED_MAX` rejects at the limit**
  - *Claim:* An update that fails validation does not mutate the live registry, and a concurrent reader never observes a partial toolset.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-tools -E 'test(registry::atomic_swap)'` — expect `failed_update_leaves_previous`, `reader_never_sees_partial`, and `rejects_at_tools_loaded_max` to pass, the last naming `TOOLS_LOADED_MAX` from `05-tools-and-extensions.md` §Limits.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(toolset::) + test(names::) + test(allowlist::) + test(registry::)'` and sees six IDs, `Task`→`Agent`, allowlist filtering, and atomic swap pass**
  - *Claim:* The registry and toolset modules pass with the baseline name comparison included.
  - *Evidence to collect:* Run the filter and confirm zero failures and that `model_facing_names_match_baseline` compared against the extracted table rather than literals.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-runtime/src/ports/tool.rs` (Task 08) defines the definition record this registry stores; confirm `ports::tool_definition_fields` still passes : ☐ (PRESERVED / REGRESSION)

## Residue

Approval policy and permission action are fields on the definition record here; their enforcement is Tasks 33–34.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
