# Task 10 verification certificate — integrated review loop 3

**Task:** [10-protocol_wire_enums_and_fixtures.md](10-protocol_wire_enums_and_fixtures.md) · **Plan:** [plan.md](../plan.md)
**State:** Validated 2026-08-14 — DONE

## Validation summary

- **Assessment:** CORRECT / DONE (high confidence).
- **Pinned source:** sibling checkout HEAD exactly `300f923f16cc8eee50656d7da732902c1dea2b65`.
- **Fixture:** 95 commands and 106 messages, respectively 95/106 unique; 6,351 bytes, 219 LF lines, final LF; SHA-256 `2a79b0578881e86f7bee38f5d4650f1f842068ff566b1b5b781437f64b912f36`.
- **Fresh selectors:** manifest 3/3 PASS; decode 6/6 PASS; exact malformed input 1/1 PASS; runtime+manifest 4/4 PASS; source hygiene 2/2 PASS; full protocol 15/15 PASS; full workspace 204/204 PASS.
- **Fresh gates:** Node syntax and extractor self-test PASS; required no-argument `--check` twice PASS; explicit pinned sibling `--source ../letta-code --check` PASS; fmt PASS; Clippy with `-D warnings` PASS; deny PASS; rustdoc with `-D warnings` PASS.
- **Default discovery:** no-argument discovery now PASSES from the repository root and prefers the sibling checkout `../letta-code`; the extractor self-test also confirms sibling preference, CI-compatible child fallback `./letta-code`, explicit source selection, and the no-source diagnostic. The previous default-path caveat is resolved and removed.

## Obligations

### O1 — Bidirectional fixture/enum equality

**Status: SATISFIED.** `cargo nextest run --manifest-path .../Cargo.toml -p lotta-protocol -E 'test(manifest::)'` passed 3/3. Independent source parsing found exactly 95 command and 106 message wire literals, all unique, ordered-equal and set-equal to the fixture. The fixture pins command `src/types/protocol_v2.ts#WsProtocolCommand`, public message entrypoint `src/types/app-server-protocol.ts` resolving to `protocol_v2.ts#WsProtocolMessage`, schema version 1, and the exact source commit. Both generated enums are internally tagged with `type`, contain one unit variant per member, and have no fallback. `APP_SERVER_PROTOCOL_VERSION = 1` is pinned and covered by the 15/15 protocol run.

### O2 — Deterministic pinned extraction and CI enforcement

**Status: SATISFIED.** `node --check tools/extract-protocol-fixture.mjs` and `node tools/extract-protocol-fixture.mjs --self-test` passed. From the repository root, required no-argument `node tools/extract-protocol-fixture.mjs --check` passed twice, and explicit `node tools/extract-protocol-fixture.mjs --source ../letta-code --check` passed without modifying the fixture. Default discovery selected the pinned sibling checkout. The fixture retained the exact count, byte shape, and SHA above.

Source inspection confirms a single iterative work stack resolves local aliases, unions/intersections, named imports, and namespace imports. Active declarations are tracked with enter/leave frames, cycle diagnostics include the declaration path, and limits are checked for 256 source files, 4,096 declarations, 4,096 union members, 256 active declarations, and 16,384 work items. The self-test exercises local alias and union cycles, named-import and namespace cycles, depth 255/256 acceptance and 257 rejection, unresolved members, duplicates, sibling-preferred default discovery, child fallback for CI, explicit source selection, and no-source failure. CI pins Node 24, checks out the source at the exact commit into `letta-code`, runs syntax/self-test, runs no-write check mode twice, and asserts the checked fixture remains git-clean.

### O3 — Decode policy

**Status: SATISFIED.** The exact decode selector passed 6/6. Unknown string tags become `DroppedUnknown` with zero state mutation and no outbound notice; malformed JSON, non-object values, and missing/non-string tags silently drop. Known tags tolerate extra fields. `input` keeps runtime and optional request ID strict, validates required payload branches and enum values, and retains baseline reason strings and branch precedence. Source inspection confirms no raw unknown command is retained.

### O4 — Recoverable malformed input

**Status: SATISFIED.** `cargo nextest run ... -E 'test(decode::malformed_input_is_non_terminal)'` passed 1/1. It proves outer `stream_delta`, inner `loop_error`, exact create-message baseline reason, `stop_reason = error`, non-terminal semantics, zero mutation, preserved runtime in the notice, `admits_further_input = true`, and acceptance of a subsequent valid input.

### O5 — Repository definition of done and source hygiene

**Status: SATISFIED.** Fresh gates passed: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; full workspace nextest 204/204; `cargo deny check` (`advisories ok, bans ok, licenses ok, sources ok`); and `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps`.

The remediated scanner is AST-based: `syn::parse_file` plus `Visit` handles free, impl, trait/default, and foreign functions, traverses item attributes, excludes only test-gated items/modules, and structurally detects direct and nested production `allow`/`cfg_attr` suppressions. Its same-path mutation matrix rejects overlong plain, multiline generic async, const/unsafe impl, trait-default, and foreign functions; verifies test-only exclusion and continuation into production; accepts braces/allow-like text in raw strings and comments; and checks direct, inner, nested conditional allow variants. The exact source-hygiene selector passed 2/2.

Actual Task10 production maxima are below hard limits: Rust files are at most 287 lines, extractor 356 lines, and the maximum line is exactly 100 UTF-8 bytes. `cargo tree -p lotta-protocol -e normal --depth 1` shows only direct normal dependencies `lotta-domain`, serde, and serde_json. Dev-only scanner dependencies are proc-macro2, serde_json, and syn; source architecture remains transport-neutral and forbids unsafe code.

### O6 — Reviewability

**Status: SATISFIED.** Fresh `cargo nextest run ... -p lotta-protocol` passed 15/15, including source hygiene 2/2 and manifest 3/3. Named manifest, decode, malformed-input, serde-enum, runtime-snapshot, source-hygiene, and mutation tests expose the required review surface. Default checks twice and the explicit pinned sibling check are clean and byte-identical, with counts 95/106 and SHA `2a79b0578881e86f7bee38f5d4650f1f842068ff566b1b5b781437f64b912f36`.

## Regression check

**PRESERVED.** Runtime snapshot plus manifest passed 4/4, full protocol passed 15/15, and complete workspace passed 204/204 with dependency-direction enforcement included. Clippy, fmt, deny, and rustdoc all passed. Verification changed no implementation, task, or VCS state; only this certificate was replaced as authorized.

## Conclusion

All O1–O6 are SATISFIED and regression is PRESERVED. The prior loop-2 hygiene concern is resolved by the syn AST scanner and its expanded mutation matrix, and the prior default-discovery caveat is gone: sibling preference passes locally while CI child fallback remains supported. Current integrated Task10 behavior, pinned fixture, decode policy, CI enforcement, architecture, and repository gates substantiate completion.

**VERDICT: CORRECT**
**COMPLETION: DONE**
**CONFIDENCE: high**
