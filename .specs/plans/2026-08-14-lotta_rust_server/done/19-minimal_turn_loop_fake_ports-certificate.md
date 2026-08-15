# Task 19 verification certificate — adversarial review loop 2

**Task:** [19-minimal_turn_loop_fake_ports.md](19-minimal_turn_loop_fake_ports.md) · **Plan:** [plan.md](../plan.md)  
**Reviewed:** 2026-08-15 against workspace `/Volumes/Delorean/code/five-letters/lotta-workspaces/task-19`  
**State:** DONE

## Verification result

I independently re-read the task, runtime/turn specification, loop-1 failure certificate, complete remediated Jujutsu diff, fixture/pin material, production sources, and all Task 19 tests. I modified only this main certificate; implementation, task, workspace metadata, and VCS were untouched.

DONE(Task 19) requires O1–O6 SATISFIED and regression PRESERVED. All are discharged below.

## O1 — projections and reasoning preservation

**Status: SATISFIED**

- Exact selector `test(turn::projections)` ran **4/4 PASS**, twice. Broad paths contain no `turn::tests::` layer.
- `anthropic_reasoning_fixture_exact_replay` parses every JSONL line from `fixtures/providers/anthropic/reasoning-redacted/baseline-events.jsonl` with Serde, maps the normalized fixture events, and drives them through real `run_turn`.
- It proves exact ordered persisted projections and matching `StreamDelta` events: reasoning `SANITIZED_FIXTURE_REASONING`, redacted marker `<redacted-fixture>`, text `SANITIZED_FIXTURE_TEXT`, then exactly one `Finished(EndTurn)`.
- The three synthetic tests independently prove exact text, reasoning, and redacted projection values, each matching its emitted delta and followed by one terminal.

## O2 — bounded end-only tool-call assembly and stable identity

**Status: SATISFIED**

- Exact selector `test(turn::tool_call_assembly)` ran **3/3 PASS**, twice.
- Production delta arm (`turn/loop.rs:241–244`) does only `state.accumulator.append(&call_id, chunk.as_slice())`; it contains no JSON parse, `.end`, or `ValidatedToolInput`. Parsing/assembly ends at `ToolCallEnd` through `execute_call`, where `accumulator.end` precedes `ValidatedToolInput::new`.
- Actual-loop malformed partial `{` reaches `ToolCallEnd`, returns `InvalidData("provider tool arguments JSON")`, executes no tool, and emits/appends nothing. The structural source assertion plus this observed end-time failure adequately proves validation occurs only at end; there is no delta-path parse.
- Stable ID `stable` is asserted in the emitted result and the next provider request's tool continuation message.
- The accumulator test admits exactly `TOOL_ARGUMENT_BYTES_MAX` (**4 MiB**) and rejects byte **4 MiB + 1** with the named limit. It uses one 4 MiB allocation and a one-byte append, not duplicate large buffers.
- `ProviderStepState::new(remaining)` reserves only the remaining whole-turn allowance. The global counter increments only after accumulator start and name insertion succeed.

## O3 — sequential local tools

**Status: SATISFIED**

- Exact selector `test(turn::sequential_tools)` ran **1/1 PASS**, twice.
- The real loop observes exactly `Eone, Xone, Etwo, Xtwo`, maximum in-flight **1**, result IDs `a`, then `b`, and continuation messages preserve that result order.
- Production requires `ParallelSafety::Sequential`; `CertifiedParallel` is explicitly rejected. Tool execution owner must be Rust and approval policy must be Never.

## O4 — terminal once, atomic effects, and stale/cancel suppression

**Status: SATISFIED**

- Exact selector `test(turn::terminal_once)` ran **6/6 PASS**, twice.
- Successful flow emits one `Finished` after the final stream delta and releases to Idle.
- `finish_turn_with_effect_after_await` performs exactly one live check, then the terminal effect and owner finish synchronously under exclusive `&mut ListenerRuntime`, with no await or recheck between them. Cancellation during the effect therefore linearizes completion; precheck cancellation suppresses the effect/release; effect failure propagates and retains ownership. The post-check lifecycle lookup/finish failures are typed and structurally unreachable absent an invariant bug because the exclusive registry borrow prevents replacement. There is no visible `Finished` + `Suppressed` path.
- Projection persist+emit and tool-result append+emit are each enclosed in one synchronous guard closure.
- Every actual await has an immediate same-token live check before effects: provider future completion, receiver completion before dispatch, and tool completion. In particular, provider completion is checked before any subsequent append/effect path.
- Cancellation during tool execution returns `Suppressed`, with provider count 1, no result/event/continuation effects, retained active ownership, and no protocol error.
- Real stale N→N+ coverage finishes N, begins N+ with a new identity, invokes `run_turn` using stale N, and observes provider/tool/effects all zero while N+ remains current Active.
- Seven malformed/error cases pass: no terminal, duplicate terminal, event after terminal, incomplete call, provider event error, provider-port stream error, and tool result with non-ToolUse stop. Each emits no `Finished` and retains ownership.
- Production turn/lease code contains no `panic!` or `expect`.

## O5 — bounds and repository definition of done

**Status: SATISFIED**

- Whole-turn tool count is global across steps. Actual call 257 is rejected before execution after provider **5**, tool **256**, appended results **256**.
- Actual step bound executes provider **256** and tool **256**, never provider step 257, then returns `LimitExceeded(TURN_STEPS_MAX)`.
- Public table names/defaults are exact: `TURN_TOOL_CALLS_MAX = 256`, `TURN_STEPS_MAX = 256`, with distinct event/counter strings in `TURN_RESOURCE_BOUNDS`. The local table test's arithmetic below/at/above assertions are tautological, but acceptance does not rely on them: the two real `run_turn` boundary tests exercise each `ResourceBound` behavior and exact counters.
- Test fakes are finite: scripted queues are consumed, provider sends finite vectors, tool outcomes are finite, and production caps turns/tools at 256. Test-only `Vec` recorders are reasonably bounded by those actual turn caps. Production has no unbounded event/result accumulator.
- No turn-loop spawn, lock-across-await, or global mutable state was found. Task 16 AST selector ran **2/2 PASS** and production contains exactly the two audited spawn sites (`spawn_scoped`, `spawn_idle_stop`).
- No bare lint suppressions were added. Changed production functions are at most 70 lines, lines format within 100 columns, and changed files are below 1,000 lines (largest Task 19 file: `loop.rs`, 427 lines).
- Public `TurnPorts`, `TurnEffectPort`, `TurnToolCatalog`, events/projections/outcomes, and `run_turn` have API/error documentation. Runtime retains one shared `RuntimeError` enum.
- Normal dependency tree contains no provider adapter dependency.

Fresh gates:

- `cargo fmt --all --check`: PASS.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
- `cargo doc --workspace --no-deps`: PASS; no warnings.
- `cargo nextest run -p lotta-runtime -E 'test(registry::architecture_scan)'`: **2/2 PASS**.
- `cargo nextest run --workspace`: **610/610 PASS**, **0 skipped**, no LEAK report.
- `cargo deny check`: PASS — advisories, bans, licenses, sources all OK.
- `cargo tree -p lotta-runtime --edges normal`: PASS; no concrete adapter crate.
- `cargo audit`: unavailable locally (`cargo: no such command: audit`); `cargo deny check` advisories passed.

## O6 — reviewability

**Status: SATISFIED**

Fresh selector observations, run twice:

| Selector | Run 1 | Run 2 |
|---|---:|---:|
| `test(turn::projections)` | 4/4 | 4/4 |
| `test(turn::tool_call_assembly)` | 3/3 | 3/3 |
| `test(turn::sequential_tools)` | 1/1 | 1/1 |
| `test(turn::terminal_once)` | 6/6 | 6/6 |
| `test(turn::)` | 25/25 | 25/25 |

`cargo nextest list -p lotta-runtime -E 'test(turn::)'` reports exactly **25** tests. No selected path contains `tests::`. Both broad runs completed without the prior custom-runtime LEAK symptom.

## Regression slice

**Status: PRESERVED**

`turn::queue_vertical_slice::queued_message_pumps_and_runs_as_next_turn` actually admits an ordinary message while occupied, observes `Queued`, finishes the occupied turn, pumps exactly the queued item, begins using that item's identity, runs real `run_turn`, and verifies queue empty plus lifecycle Idle. It passed in both broad runs and the 610-test workspace run.

## Commands and counts

```text
cargo nextest run -p lotta-runtime -E 'test(turn::projections)'          4/4 PASS ×2
cargo nextest run -p lotta-runtime -E 'test(turn::tool_call_assembly)'  3/3 PASS ×2
cargo nextest run -p lotta-runtime -E 'test(turn::sequential_tools)'    1/1 PASS ×2
cargo nextest run -p lotta-runtime -E 'test(turn::terminal_once)'       6/6 PASS ×2
cargo nextest run -p lotta-runtime -E 'test(turn::)'                   25/25 PASS ×2
cargo nextest run -p lotta-runtime -E 'test(registry::architecture_scan)' 2/2 PASS
cargo nextest run --workspace                                         610/610 PASS
cargo fmt --all --check                                                PASS
cargo clippy --workspace --all-targets --all-features -- -D warnings   PASS
cargo doc --workspace --no-deps                                        PASS
cargo deny check                                                       PASS
cargo tree -p lotta-runtime --edges normal                             PASS
cargo audit                                                            UNAVAILABLE
```

## Conclusion

O1–O6 are SATISFIED and regression is PRESERVED.

**VERDICT: CORRECT**  
**CONFIDENCE: high**  
**DONE: yes**
