# Done Certificate — Task 70: WebSocket agent management command group

**Task:** [70-ws_agent_management_commands.md](70-ws_agent_management_commands.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-23 — done

> Verification protocol for Task 70. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 70) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** create shortcut plus list, retrieve, create, update, and delete for agents, backed by the Task 28 query behaviors.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not return success before agent creation's side effects complete: `01-domain-model.md` §Agent requires them before success is returned.

## Obligations

- **O1 — All six agent management commands decode, route, and respond, with fixture round-trip coverage**
  - *Claim:* Each command in the Agent management row has a passing case.
  - *Evidence collected:* `cargo nextest run -p lotta-app-server -E 'test(ws::agents::commands)'` passed all 13 selected behavior tests, including the six command response paths. `cargo nextest run -p lotta-app-server -E 'test(ws::agents::fixture_round_trip)'` passed 7/7: one exact-group inventory check plus one round-trip test for each of `create_agent`, `agent_list`, `agent_retrieve`, `agent_create`, `agent_update`, and `agent_delete`.
  - *Status:* ☑ SATISFIED

- **O2 — Agent creation with local MemFS completes all four side effects before returning success**
  - *Claim:* The Git-memory tag, initialized memory files, the created repository, and the compiled default prompt all exist when the response is sent.
  - *Evidence collected:* `cargo nextest run -p lotta-app-server -E 'test(ws::agents::create_side_effects)'` passed 5/5. `AgentsBridge::create_core` saves the already-tagged record, awaits `init_memory_repo` (memory files and Git repository), then awaits `compile_default_prompt`; only after it returns does `create` emit the success response. `creation_completes_all_four_side_effects_before_the_response` observes the tag, initialized persona file, `.git` repository, and persisted prompt record from the response boundary.
  - *Checks:* Prompt compilation at `crates/lotta-app-server/src/ws/groups/agents.rs:687` completes before `create_core` returns and before response emission at lines 657–671.
  - *Status:* ☑ SATISFIED

- **O3 — List ordering is deterministic and the filters behave as §Required query patterns specifies**
  - *Claim:* Repeated list calls return identical order and each filter narrows correctly.
  - *Evidence collected:* `cargo nextest run -p lotta-app-server -E 'test(ws::agents::listing)'` passed 6/6: repeated deterministic ordering, case-insensitive name, query over description/id/model text, all-tags matching, hidden visibility, and `after` cursor continuation. The list handler maps filters into Task 28 `AgentFilters` and serves `query_agents` results without reordering.
  - *Status:* ☑ SATISFIED

- **O4 — An absent agent or a wrong local prefix returns 404, and `AGENTS_MAX` rejects creation at the limit**
  - *Claim:* Both lookup failures are 404-class and the creation cap rejects above the limit.
  - *Evidence collected:* `cargo nextest run -p lotta-app-server -E 'test(ws::agents::errors)'` passed 10/10. The set includes absent retrieve/update/delete and wrong-prefix 404-class paths, below/at/above cap boundaries, unknown shortcut model, and compaction rejection/coercion paths. The cap check precedes record or MemFS side effects, and the above-cap test confirms no leaked record.
  - *Status:* ☑ SATISFIED

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence collected:* `cargo fmt --all --check`, strict workspace Clippy, `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps`, and `cargo deny check` all exited 0. Workspace health passed 2051/2051 with only the established pinned-SHA (`conformance`, `cross_runtime`, `slice`), extension process-certificate, and provider-host-without-dedicated-env exclusions. Full package runs passed `lotta-app-server` 414/414, `lotta-store` 206/206, and `lotta-memfs` 83/83. `task14_listener` passed 4/4 with `LOTTA_BUN=/Users/stan/.bun/bin/bun` and `LOTTA_PI_AI_ROOT=/Volumes/Delorean/code/five-letters/letta-code/node_modules/@earendil-works/pi-ai`. Cumulative diff inspection found all added functions at most 70 lines, no lines over 100 columns, no new lint suppressions or panic `unwrap`/`expect` in production, and bounded numeric constants use units-last names (`AGENT_LIST_DEFAULT_ITEMS`, `PIN_WRITE_ATTEMPTS`).
  - *Status:* ☑ SATISFIED

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-app-server -E 'test(ws::agents::)'` and sees six commands, complete creation side effects, deterministic filtered listing, and the 404 and cap paths pass**
  - *Claim:* The agent management group passes.
  - *Evidence collected:* `cargo nextest run -p lotta-app-server -E 'test(ws::agents::)'` selected 41 tests and passed 41/41 with zero failures. The reviewable group contains 13 command behaviors (including all six command paths), 5 creation-side-effect tests, 6 listing tests, 10 errors tests, and 7 fixture round-trips.
  - *Status:* ☑ SATISFIED

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-store/src/query/` (Task 28) serves the listing; `cargo nextest run -p lotta-store -E 'test(query::deterministic_order)'` passed 1/1: ☑ PRESERVED

## Residue

Conversation-scoped commands are Task 71.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☑ DONE
CONFIDENCE: ☑ high
SUMMARY: Round-4 final verification reviewed the cumulative implementation and repair chain `a2de497a37cc` → `b1248c1d88ea` → `79495a53961c` → `a1f2c11ae262` → `0489940c035b`. The two final fixes are present: array null elements render empty while top-level null remains `"null"` (`String(["all", 42, null]) == "all,42,"`), and the compaction coercion test is 55 lines with plain async helpers while the command selector remains 13 tests. Exact Task 70 selectors passed commands 13/13, side effects 5/5, listing 6/6, errors 10/10, fixture round-trips 7/7, and broad agents 41/41; store deterministic ordering passed 1/1. Full app-server/store/MemFS packages passed 414/414, 206/206, and 83/83. Format, Clippy, rustdoc, and deny gates passed, and workspace health passed 2051/2051 under the established environmental exclusion protocol. The separately required `task14_listener` run passed 4/4 with the pinned Bun and pi-ai environment. Source and trace inspection confirms all six commands decode/route/respond, creation awaits all four side effects before response, listing is deterministic and filtered, 404 and `AGENTS_MAX` boundaries are covered, and the cumulative shape has ≤70-line functions, ≤100-column lines, units-last bounded constants, no new lint suppressions, and no production panic unwrap/expect.
