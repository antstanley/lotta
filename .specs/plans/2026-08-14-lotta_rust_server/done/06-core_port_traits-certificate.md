# Task 06 Certificate

**Task:** [06-core_port_traits.md](06-core_port_traits.md) · **Plan:** [plan.md](../plan.md)
**State:** Validated 2026-08-14 — final clean certification after remediation loop 3

## Definition

DONE(Task 06) requires O1–O6 SATISFIED and regression PRESERVED. Evidence below was collected independently from the complete current Jujutsu diff (13 files, 1,722 insertions, 7 deletions); prior reports were treated only as leads.

## Obligations

### O1 — Task-06 effects and minimal port surface

**Status: SATISFIED**

- `lotta-runtime::ports` exports exactly seven responsibility-backed object-safe traits: `IdGenerator`, `AgentStore`, `ConversationStore`, `TranscriptStore`, `MemFsPort`, `SandboxPort`, and `ChildProcessPort`; canonical `lotta_domain::Clock` is re-exported rather than duplicated.
- The traits contain exactly 33 minimal methods (4 + 4 + 4 + 3 + 13 + 1 + 1). MemFS remains exactly 13 canonical operations.
- Filesystem, process, random/ID, Git/revision, and time effects are represented by ports or the Clock re-export. Production runtime source contains no inline filesystem/process/random/direct-clock effect.
- No premature provider, tool, network, adapter, push, or composition abstraction was found.
- MemFS `status` explicitly supplies the committed revision for prompt freshness and assigns comparison/recompilation to runtime; `commit` explicitly requires current pre-commit Markdown/frontmatter validation by the adapter without adding a fourteenth operation.

### O2 — Dependency direction

**Status: SATISFIED**

- Live Cargo metadata rejects the exact seven forbidden adapter crates.
- The mutation fixture independently injects and rejects every forbidden crate.
- Exact selector `cargo nextest run -p lotta-runtime -E 'test(ports::dependency_direction)'` selected 1 test and passed 1/1.
- `cargo tree -p lotta-runtime --edges normal` contains only `lotta-domain`, `thiserror`, `tokio`, and `tokio-util` as direct normal dependencies; no adapter crate appears.

### O3 — Streaming and cancellation

**Status: SATISFIED**

- All six logical multi-value reads—agent list, conversation list, transcript load, tree, history, and diff—use bounded `tokio::sync::mpsc::Sender` plus `CancellationToken`.
- Both process event methods use bounded `Sender<ProcessEvent>` plus `CancellationToken`.
- No unbounded channel, receiver-return stream, raw/nested collection output, or hidden collection result bypass appears in the exported port surface.
- Broad ports selector passed 11/11.

### O4 — Public contracts and external implementation

**Status: SATISFIED**

- All seven traits are `Send + Sync`, object-safe, and return the shared boxed `Send` `PortFuture`.
- The public surface is externally constructible/inspectable and an independent temporary adapter crate compiled against it.
- All ports use one public `RuntimeError`. Separate inspection found a finite documented enum with stable textual category prefixes and scrubbed adapter failure fields; no Task-06 obligation requires downstream exhaustive matching stability beyond this boundary.
- Trait documentation substantively covers preconditions, errors, cancellation, and ownership.
- Process contracts preserve empty argv/environment-value compatibility and assign NUL/platform-specific program, argument, environment-name, and environment-value validation/translation to adapters.
- Path contracts cover UTF-8, lexical absolute/relative shape, traversal, byte/component ceilings, retained root, root equality, existing targets, absent create/write/rename targets, parent canonicalization, unresolved-component validation, no-follow/handle-relative operations, symlink re-checks, and TOCTOU ownership.

### O5 — Definition of done and hard limits

**Status: SATISFIED**

- Purpose wrappers use private storage and public validating constructors/accessors. Required identifiers reject empty input; empty arguments, environment values, and optional descriptions remain compatible. Nested initial-memory values are converted before retention.
- Canonical MemFS values are exactly 8 MiB per file and 100,000 files.
- All 19 runtime-owned `ResourceBound` rows have an exact drift fixture covering name, value, event, units-last counter, `Reject`, and below/at/over observability. Domain remains exactly nine Task-05 bounds.
- Compile-time structural tests destructure every changed exported boundary newtype storage and all changed process request/event/outcome, MemFS status/tree/history, and transcript fields/variants with exact types. Nested `ProcessRequest` redaction is verified through entry, environment, and request formatting.
- Exact bounds/structural/redaction/path/Clock/dependency suite passed 9/9. Path tests cover byte/component boundaries, empty/dot/traversal/root behavior, retained root/root equality, create-target parent shape, and non-UTF-8 rejection.
- Full workspace nextest passed 97/97. Formatting passed. Workspace/all-target/all-feature Clippy with denied warnings passed. `cargo deny check` passed. Workspace/all-feature rustdoc with denied warnings passed.
- Production audits found no panic/unwrap/expect, unsafe code, direct clock, raw effect call, unbounded channel, unnecessary runtime dependency, or duplicate abstraction. All production Rust lines are at most 100 columns.
- The only function over 70 lines is the deliberately explicit 19-row test fixture, not production implementation; it is locally justified and does not violate the implementation-function limit.

### O6 — Reviewability

**Status: SATISFIED**

- Exact dependency selector is nonzero and green; broad ports is green.
- Live metadata/tree, all-seven mutation enforcement, compile-time structural field checks, public adapter compilation, source/effect scan, public-error inspection, bound placement, line-length, and function-length audits are independently reproducible.

## Regression check

**PRESERVED.** Full workspace passed 97/97, including Task-02–05 schema, bound, runtime-state, scope, timestamp normalization, secret-redaction, and composition-root coverage. Domain Task-05 bounds remain exactly nine. Formatting, Clippy, deny, rustdoc, dependency policy, and external adapter compilation passed. No established caller or architectural regression was found.

## Residue

Provider/network effects remain Task 07, tool effects Task 08, and concrete composition later. No early implementation of those responsibilities was found.

## Conclusion

- **Correctness: CORRECT**
- **Completeness: DONE**
- **Confidence: high**

O1–O6 are SATISFIED and regression is PRESERVED. No remaining remediation is required for Task 06.
