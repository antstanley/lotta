# Done Certificate — Task 43: MCP client, transports, and bounded discovery

**Task:** [43-mcp_client.md](43-mcp_client.md) · **Plan:** [plan.md](../plan.md)
**State:** Authored 2026-08-14 — unverified

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
  - *Status:* ☐ unverified

- **O2 — Discovered tools are namespaced per server and bounded by `MCP_TOOLS_PER_SERVER_MAX` and `MCP_SERVERS_PER_AGENT_MAX`**
  - *Claim:* Two servers exposing the same tool name do not collide, and both caps reject above their limit.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(mcp::discovery)'` — expect `namespaces_per_server`, and below/at/above cases for both caps naming the §Limits constants.
  - *Checks:* Resolve the cap constants — confirm they are `MCP_SERVERS_PER_AGENT_MAX` and `MCP_TOOLS_PER_SERVER_MAX` from `05-tools-and-extensions.md` §Limits, not a generic `MCP_SERVERS_MAX`.
  - *Status:* ☐ unverified

- **O3 — A server refresh replaces its tool group atomically, and a failed refresh leaves the previous group intact**
  - *Claim:* During refresh no partial group is visible, and a failure is a no-op on the registry.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(mcp::refresh)'` — expect `atomic_group_swap` and `failed_refresh_is_noop` to pass.
  - *Status:* ☐ unverified

- **O4 — OAuth tokens and server credentials live in the credential store and never reach diagnostics**
  - *Claim:* A captured log across connect, discover, and failure contains no token value.
  - *Evidence to collect:* Run `cargo nextest run -p lotta-extensions -E 'test(mcp::credentials)'` — expect PASS; the test asserts the marker token is absent from captured tracing output and from any error rendering.
  - *Status:* ☐ unverified

- **O5 — Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)**
  - *Claim:* The repo-wide gates named in the plan's definition-of-done baseline pass for this change.
  - *Evidence to collect:* Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and `cargo deny check` — expect exit code 0 from each. Read every constant this task introduces and confirm it is a `const` whose name puts units last (`development-guidelines.md` §Naming), and that each function the task added stays within 70 lines and 100 columns.
  - *Status:* ☐ unverified

- **O6 — Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(mcp::)'` and sees three transports, stdio defaulting, namespaced bounded discovery, atomic refresh, and credential containment pass**
  - *Claim:* The MCP module passes.
  - *Evidence to collect:* Run the filter and confirm zero failures and the `omitted_type_defaults_to_stdio` case.
  - *Status:* ☐ unverified

## Regression check

For each unit this task changes, the validator traces one downstream caller:

- `crates/lotta-extensions/src/sidecar/framing.rs` (Task 42) validates the stdio child boundary; confirm `sidecar::validation` still passes with the MCP child attached : ☐ (PRESERVED / REGRESSION)

## Residue

MCP server lifecycle supervision reuses the Task 42 supervisor; no separate restart policy is introduced here.

## Conclusion

<!-- Validator derives this from the obligation statuses and the regression check, per the rubric. -->

Rubric — **NOT_DONE** if any load-bearing obligation is UNSATISFIED or the regression
check found a REGRESSION; **PARTIAL** if every obligation is SATISFIED except one or
more UNVERIFIED and no regression; **DONE** only if every obligation is SATISFIED and
the regression check is PRESERVED.

VERDICT: ☐ (DONE | PARTIAL | NOT_DONE)
CONFIDENCE: ☐ (high | medium | low)
SUMMARY: ☐
