# Task 09 — Testkit and port contract suite certificate

**Task:** [09-testkit_and_port_contract_suite.md](09-testkit_and_port_contract_suite.md) · **Plan:** [plan.md](../plan.md)
**State:** Independently validated final review loop 3, 2026-08-14 — DONE

## Definition and scope

DONE(Task 09) requires O1–O6 SATISFIED and Task02/04/06–08 regression PRESERVED. This
fresh clean verifier independently read the current 32-file Jujutsu diff (5,452 insertions, 31
deletions), Task09 task/certificate, governing references, and changed source. Only this certificate
was edited; implementation, task, VCS state, and file placement were not modified.

The canonical ten are:

1. `Clock` → `FakeClock` → `clock_contract`
2. `IdGenerator` → `FakeIdGenerator` → `id_generator_contract`
3. `AgentStore` → `FakeAgentStore` → `agent_store_contract`
4. `ConversationStore` → `FakeConversationStore` → `conversation_store_contract`
5. `TranscriptStore` → `FakeTranscriptStore` → `transcript_store_contract`
6. `MemFsPort` → `FakeMemFs` → `memfs_contract`
7. `SandboxPort` → `FakeSandbox` → `sandbox_contract`
8. `ChildProcessPort` → `FakeChildProcess` → `child_process_contract`
9. `ProviderPort` → `FakeProvider` → `provider_contract`
10. `ToolPort` → `FakeTool` → `tool_contract`

One `port_matrix!` row source generates `PortKind`, both exact wrapper lists, helper mapping, and
trait coercions. Exact selectors passed: contract 10/10; fakes 14/14 (ten canonical wrappers plus
four focused fake bounds tests); clock/IDs 4/4 twice; fixtures 11/11; roots 7/7; source audit/hard
limits 5/5; full testkit 50/50 twice; full workspace 189/189.

## Obligations

### O1 — One reusable shared contract per trait: SATISFIED

The ten public async generic suites are split by concern. Contract source has zero concrete `Fake*`
or `fakes` imports. Store/MemFS factories create clean instances; process/provider use generic
scenario factories. Tool uses public adapter-neutral `ToolContractCase`, `ToolContractProbe`, and
redaction-safe snapshots, permitting a real-adapter test wrapper to publish observations without
implementing fake-only control traits. Fake-only setup/counter boundary hooks remain under fake tests.
Assertions are owned by contract suites; generated wrappers only invoke mapped helpers. The single
matrix prevents drift among ten kinds, both wrapper lists, helpers, adapters, traits, and coercions.

### O2 — Correct bounded fakes and sensitive semantics: SATISFIED

Stores preserve replacement, scope, ordering, absence, transcript initialization/append/load,
bounded streaming, backpressure, cancellation, and closed-receiver behavior. Clock and all five ID
methods prove deterministic exact traces and checked exhaustion. MemFS exercises all thirteen port
operations, atomic conflicts, history and worktree semantics, stream behavior, and missing values.
Aggregate snapshot cardinality/retained-byte checks run before clones. Diff computes every fixed and
payload byte with checked addition, rejects before allocation, allocates exactly once, and avoids
`format!`. Tool covers eight normalized outcomes plus adapter failure, exact request projection,
redaction, exact streaming JSON byte counting with O(1) retained state, pending-response preservation,
cancellation, sequential admission, and checked atomic counters. Process/provider use no timer;
provider covers all ten event variants and legal Stop or Error terminals while preserving Task07's
eight invariants.

### O3 — Deterministic clocks/IDs and forbidden-effect proof: SATISFIED

Lifecycle owner UUID generation is injected through `IdGenerator`; deterministic IDs retain valid UUID
v4 bits without UUID's `v4` feature, `getrandom`, or normal entropy. Clock nanoseconds and ID counters
use checked arithmetic. The recursive syn analyzer scans production source, skips cfg-test scopes,
tracks top-level/block-local grouped, renamed, scoped, and filesystem-glob imports, rejects `include!`,
resolves qualified calls and known receiver paths, and avoids false positives for ordinary
`connect`/`bind`/`spawn` methods. Its mutation matrix covers each effect and prior bypass class. The
normal-tree audit runs `cargo tree --offline`, and the resolved normal tree contains no forbidden
network/random/tempfile/adapter family. The analyzer follows the split contract tree recursively, so
that split creates no blind spot. Test-only cargo-tree execution is offline and does not exercise a
network path.

### O4 — Confined fixtures and temporary roots: SATISFIED

Fixtures passed 11/11: canonical cwd-independent root, lexical confinement, absolute/traversal and
symlink rejection, distinct UTF-8/JSON/missing errors, exact streaming limits, confined diagnostics,
and documented static fixture-tree race assumption. Roots passed 7/7: validated labels, PID plus
checked atomic sequence, atomic creation with bounded collision retries, identity marker, explicit
idempotent cleanup, Drop cleanup, and replacement/marker safety without absolute path leakage.

### O5 — Repository definition of done: SATISFIED

Fresh gates passed: `cargo fmt --all -- --check`; workspace/all-target/all-feature Clippy with
`-D warnings`; `cargo deny check`; rustdoc with `-D warnings`; full workspace 189/189; full testkit
50/50 twice. `cargo tree -d` was run separately and printed no duplicates. All 21 testkit Rust source
files are <=1,000 lines (maximum 840), all lines are <=100 bytes, and production functions are <=70
lines via syn span checks. The same hard-limit gate has file/line/function/allow mutations. Production
has zero `allow` suppressions and the structural gate found no production panic/unwrap/expect/unsafe,
recursion, or direct forbidden effects outside the fixture/root allowance. `TestkitError` is the one
public testkit error type.

### O6 — Reviewability: SATISFIED

The exact named selectors are nonzero and independently discoverable: contract 10, fakes 14,
clock/IDs 4 twice, fixtures 11, roots 7, source audit 5, and testkit 50 twice. Public contracts are
adapter-generic and fixture constructors are public. Source, dependency, effect, hard-limit, and matrix
gates make drift visible to a reviewer.

## Regression check

**Status: PRESERVED**

The complete workspace passed 189/189. This includes Task02 ID/scalar/scope tests; Task04 lifecycle,
lease, and transition tests; Task06 exact nineteen bounds, object safety, structural effects, and
dependency-direction tests; Task07 provider 16-case surface plus exact ten events and all eight
streaming invariants; and Task08 exact fields/default, eight outcomes, ownership, object safety,
boundaries, and broad contract. Provider Task09 traces use legal Stop-or-Error terminals. Normal UUID
v4/getrandom generation is absent while injected deterministic UUID-v4 values preserve lifecycle
semantics.

## Conclusion

- **Correctness: CORRECT**
- **Completeness: DONE**
- **Confidence: high**

O1–O6 are SATISFIED and regression is PRESERVED. Every prior issue was rechecked: concrete contract
fake coupling and fake control bounds are gone; matrix coverage is generated from one source; MemFS
pre-allocation and aggregate-clone bounds are fixed; tool counters are checked; include/local-alias and
resolved-tree audit gaps are closed; contract modules meet file/function/line/suppression limits; and
`cargo tree -d` was independently executed.
