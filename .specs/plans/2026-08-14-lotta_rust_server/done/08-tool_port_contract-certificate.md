# Done Certificate — Task 08: Tool port contract

**Task:** [08-tool_port_contract.md](08-tool_port_contract.md) · **Plan:** [plan.md](../plan.md)
**State:** Independently verified final, 2026-08-14 — DONE

## Definition

DONE(Task 08) means O1–O6 are SATISFIED and the Task06/Task07 regression check is
PRESERVED. This certificate records fresh evidence collected directly from the Task08 Jujutsu
workspace without modifying implementation, task text, VCS state, or any file except this
certificate.

## Verification scope

The verifier independently read:

- the complete four-file Jujutsu diff;
- `08-tool_port_contract.md` and this certificate;
- `05-tools-and-extensions.md`, `architecture-principles.md`,
  `development-guidelines.md`, and the relevant runtime/open-question references;
- all of `ports/tool.rs`, `ports/tool_tests.rs`, the touched `ports/mod.rs`, and `bounds.rs`.

The final diff is 1,296 inserted lines across `bounds.rs`, `ports/mod.rs`, new `ports/tool.rs`,
and new `ports/tool_tests.rs`.

## Obligations

### O1 — Eleven typed definition fields and validated external types

**Status: SATISFIED**

`ToolDefinition` has exactly eleven public, distinctly typed fields:

1. `InternalToolName`
2. `ModelFacingToolName`
3. `ToolInputSchema`
4. `ToolDescriptionAsset`
5. `ToolExecutionOwner`
6. `ToolApprovalPolicy`
7. `PermissionAction`
8. `ParallelSafety`
9. `ToolTimeout`
10. `ToolOutputLimit`
11. `SecretRedactionSpec`

The normal constructor takes the ten caller-owned inputs and fixes the eleventh field to
`Sequential`. Purpose-specific bounded newtypes prevent generic metadata/string leakage. A fresh
temporary external crate compiled while constructing the public schema, redaction, definition,
input, request, and `Arc<dyn ToolPort>` implementation.

Fresh exact selector:

- `test(ports::tool_definition_fields)`: **1/1 PASS**

`ToolInputSchema` is private-storage, top-level-object-only, bounded by exact compact JSON size,
and uses the same validation path for constructors and serde. It accepts `{}`, optional
`type: "object"`, object `properties`, object-valued and boolean property schemas, and valid
`required` arrays. It rejects scalar/array top levels, non-object type, malformed
`properties`/`required`, non-string required entries, duplicate required names, and—when
`properties` exists—required names absent from it. Constructor and serde adversarial cases both
pass.

`SecretFieldPath` validates a nonempty absolute JSON Pointer, nonempty segments, only `~0`/`~1`
escapes, no NUL, UTF-8 byte length at 255/256 boundaries, exact uniqueness, bounded field count,
and serde reconstruction. The redaction record combines the bounded field set and policy.

### O2 — Conservative parallel certification

**Status: SATISFIED**

`ParallelSafety::default()` is `Sequential`; `ToolDefinition::new` always writes `Sequential`.
Parallel execution requires an explicit, nonempty, bounded `ParallelCertificationId` through
`with_parallel_certification`. No scheduler, concurrency policy, or premature parallel behavior
was added. The enum retains only sequential and explicitly certified states.

Fresh exact selector:

- `test(ports::parallel_safety_defaults_sequential)`: **1/1 PASS**

### O3 — Eight durable tool outcomes

**Status: SATISFIED**

`ToolOutcome` has exactly eight snake_case-tagged variants: success, user denial, interruption,
timeout, validation failure, sandbox denial, spawn failure, and tool-defined error. Timeout is the
tool outcome and remains distinct from provider timeout. All variants round-trip, unknown tags are
rejected, and bounded payload newtypes reject malicious overbound fields.

Fresh exact selector:

- `test(ports::tool_outcome_round_trip)`: **8/8 PASS**

`ToolOutputLimit` validates nonzero byte and Unicode-scalar ceilings through both constructor and
serde. `ToolResultText` validates both dimensions, privately preserves the exact lower applied
limit, serializes it on wire, and revalidates malicious values and embedded limits on decode.
There is no `ToolResultBytes` definition or export.

### O4 — Exact execution owners

**Status: SATISFIED**

`ToolExecutionOwner` has exactly Rust, MCP, controller, mod sidecar, and channel gateway, with
stable snake_case serde names and no unknown/unowned/vendor fallback.

Fresh exact selector:

- `test(ports::execution_owner)`: **1/1 PASS**

### O5 — Repository definition of done

**Status: SATISFIED**

Fresh gates:

- schema/adversarial and broad tool selector: **4/4 PASS**;
- exact Task08 resource-bound selector: **1/1 PASS**;
- full workspace nextest, all features: **139/139 PASS**;
- `cargo fmt --all --check`: **PASS**;
- Clippy workspace/all targets/all features with `-D warnings`: **PASS**;
- `cargo deny check`: **PASS** (`advisories`, `bans`, `licenses`, `sources` all OK);
- rustdoc workspace/all features/no dependencies with `-D warnings`: **PASS**;
- normal runtime dependency tree: **PASS**;
- duplicate dependency tree: **no duplicates printed**;
- temporary external consumer crate: **cargo check PASS**.

The seven tool bounds have exact names, values, metadata, stable order, `Reject` behavior, and
below/at/over observations:

1. `TOOL_INPUT_BYTES_MAX = 4 MiB`
2. `TOOL_RESULT_BYTES_MAX = 1 MiB`
3. `TOOL_RESULT_MODEL_CHARS_MAX = 32,000`
4. `EXTERNAL_TOOL_CALL_TIMEOUT_MS = 300,000`
5. `TOOL_NAME_BYTES_MAX = 256`
6. `TOOL_DESCRIPTION_BYTES_MAX = 64 KiB`
7. `TOOL_SECRET_FIELDS_ITEMS_MAX = 128`

Owning constructors enforce the applicable bounds. Input/schema JSON measurement streams into a
bounded O(1)-space counter rather than allocating a serialization buffer. The request owns its
validated input and full definition/owner context, cancellation token, and bounded deadline;
request/input Debug redacts payload content. `ToolPort` is object safe and externally implementable.

Structural audits found:

- no production panic/unwrap/expect/unsafe, recursion, direct clock, sleep, or unbounded channel;
- no secrets exposed in request/input Debug;
- no raw public bound or collection leak for bounded contract values;
- no `ToolResultBytes`, duplicate Task08 contract, vendor type, or added dependency;
- no premature registry/toolset/allowlist/executor/policy/sandbox/hook/clamp behavior;
- no bare TODO, missing public docs, broad suppression, or unjustified lint suppression;
- the sole production lint allowance is narrowly reasoned for the exact ten-input constructor;
- no line over 100 columns and no touched Rust file over 1,000 lines;
- production functions remain at or below 70 lines.

### O6 — Reviewable selector

**Status: SATISFIED**

Fresh reviewer selectors produce the required **1/1/8/1** exact counts. The broad selector
`test(ports::tool_contract)` passed **4/4**, including its aggregate contract, external object
safety, deadline/output boundaries, and schema/adversarial serde case. The aggregate invokes path,
redaction, lower-limit serde, all eight outcomes, exact owners/fields, sequential certification,
and owning-boundary checks.

## Regression check

**Status: PRESERVED**

Fresh Task06 evidence:

- `test(ports::dependency_direction)`: **1/1 PASS**;
- exact nineteen-row Task06 bounds: **1/1 PASS**;
- `RUNTIME_RESOURCE_BOUNDS` remains exactly 19 rows.

Fresh Task07 evidence:

- broad `test(ports::provider)`: **16/16 PASS**;
- exact seven-row provider bounds: **1/1 PASS**;
- `PROVIDER_RESOURCE_BOUNDS` remains exactly 7 rows.

The full 139-test workspace run independently confirms both prior task surfaces remain green.

## Decisions and residue

- A required list without `properties` remains valid JSON Schema; membership is enforced when a
  `properties` object is present.
- Boolean property schemas are valid and intentionally accepted.
- JSON Pointer spellings are validated, not decoded into a second canonical representation;
  valid RFC 6901 escaping remains injective for exact duplicate detection.
- Toolset IDs/name maps/allowlists belong to Task32; execution pipeline and family clamps belong to
  Task33; concrete built-ins and external adapters belong to later tasks.

## Conclusion

All O1–O6 are backed by fresh source inspection and passing execution evidence. Task06 and Task07
regressions are PRESERVED. The previous duplicate/mismatched `required` concern is fixed in the
shared constructor/serde validation path and covered adversarially.

VERDICT: **DONE**
CORRECTNESS: **CORRECT**
COMPLETION: **DONE**
CONFIDENCE: **high**
SUMMARY: Exact fields/owners/outcomes/selectors, schema and path validation, sequential
certification, object-safe bounded port contracts, output-limit wire preservation, seven exact
resource bounds, broad adversarial coverage, all repository gates, external compilation, and
Task06/07 regressions all pass.
