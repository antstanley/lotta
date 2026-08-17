# Done Certificate — Task 43: MCP client, transports, and bounded discovery

**Task:** [43-mcp_client.md](43-mcp_client.md) · **Plan:** [plan.md](../plan.md)
**State:** Verified 2026-08-16 — DONE

> Verification protocol for Task 43. A validating agent discharges it: collect each
> obligation's evidence, run its checks, set the Status, then derive the Conclusion by the
> rubric. Do not mark an obligation SATISFIED without its evidence; do not record DONE with
> any non-SATISFIED obligation.

## Definition

DONE(Task 43) ≡ every obligation O1…O6 below holds, each backed by the evidence the
obligation names (a file location, a named test result, or an execution trace) — not by assertion.

## Premises

- **P1 — Goal.** a Rust MCP client supporting `stdio`, `sse`, and `http` with namespaced bounded discovery and atomic per-server refresh.
- **P2 — Obligations.** Done iff O1…O6 all hold, one per definition-of-done item in DoD order; O6 is the `Reviewable:` item.
- **P3 — Invariants.** Must not collapse the three transports into one: `05-tools-and-extensions.md` §External tools and MCP names `stdio`, `sse`, and `http` explicitly, with `stdio` as the omitted default.

## Obligations

- **O1 — All three transports work and an omitted discriminant defaults to `stdio`**
  - *Claim:* `stdio`, `sse`, and `http` each connect and discover, and a config with no `type` is treated as `stdio`.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(mcp::transport)'` — expect three transport cases plus `omitted_type_defaults_to_stdio`, matching `05-tools-and-extensions.md` §External tools and MCP.
  - *Status:* ☒ SATISFIED — 5/5 transport cases exercise distinct public-factory stdio, persistent SSE, and streamable HTTP adapters plus omitted-type stdio; stdio uses the Task 42 envelope and lifecycle.

- **O2 — Discovered tools are namespaced per server and bounded by `MCP_TOOLS_PER_SERVER_MAX` and `MCP_SERVERS_PER_AGENT_MAX`**
  - *Claim:* Two servers exposing the same tool name do not collide, and both caps reject above their limit.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(mcp::discovery)'` — expect `namespaces_per_server`, and below/at/above cases for both caps naming the §Limits constants.
  - *Checks:* Resolve the cap constants — confirm they are `MCP_SERVERS_PER_AGENT_MAX` and `MCP_TOOLS_PER_SERVER_MAX` from `05-tools-and-extensions.md` §Limits, not a generic `MCP_SERVERS_MAX`.
  - *Status:* ☒ SATISFIED — production manager evidence covers reversible per-server namespaces, actual 63/64/65 server and 511/512/513 tool boundaries, concurrent cap admission, pagination, schema, and cursor bounds.

- **O3 — A server refresh replaces its tool group atomically, and a failed refresh leaves the previous group intact**
  - *Claim:* During refresh no partial group is visible, and a failure is a no-op on the registry.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(mcp::refresh)'` — expect `atomic_group_swap` and `failed_refresh_is_noop` to pass.
  - *Status:* ☒ SATISFIED — 2/2 refresh cases prove concurrent readers observe complete old or new groups only, successful replacement closes the prior session, and every failed candidate is an exact registry/group/session no-op.

- **O4 — OAuth tokens and server credentials live in the credential store and never reach diagnostics**
  - *Claim:* A captured log across connect, discover, and failure contains no token value.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(mcp::credentials)'` — expect PASS; the test asserts the marker token is absent from captured tracing output and from any error rendering.
  - *Status:* ☒ SATISFIED — 4/4 credential cases prove just-in-time redacted HTTP/SSE/stdio resolution, URL containment, OAuth compare-and-swap, secret bounds, and marker-free captured diagnostics across success and failure.

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☒ SATISFIED — workspace tests 1,345/1,345, build, fmt, strict all-target/all-feature Clippy, private Rustdoc, and deny pass; independent parsing found 350/350 touched functions within limits.

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(mcp::)'` and sees three transports, stdio defaulting, namespaced bounded discovery, atomic refresh, and credential containment pass**
  - *Claim:* The MCP module passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `omitted_type_defaults_to_stdio` case.
  - *Status:* ☒ SATISFIED — clean reviewer ran 24/24 MCP cases and found no remaining framing, protocol, routing, lifecycle, refresh, credential, bound, shape, or regression defect.

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-extensions/src/sidecar/framing.rs` (Task 42) validates the stdio child boundary; confirm `sidecar::validation` still passes with the MCP child attached : ☒ PRESERVED — 12/12 validation cases pass through the shared Task 42 implementation.

## Residue

MCP server lifecycle supervision reuses the Task 42 supervisor; no separate restart policy is introduced here.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☒ DONE
CONFIDENCE: ☒ high
SUMMARY: MCP now provides bounded Task 42-framed stdio, persistent legacy SSE, and resumable streamable HTTP clients; atomic namespaced discovery preserves every registry source, and credential values remain store-contained and diagnostic-free across all certified paths.
