# Task 34 — Permission modes, file-backed scopes, and shell analysis

**Plan:** [plan.md](../plan.md) · **Certificate:** [34-permissions_model-certificate.md](34-permissions_model-certificate.md)

**Implements:** [05-tools-and-extensions.md §Permissions and sandbox](../../../05-tools-and-extensions.md#permissions-and-sandbox)
**Depends on:** 27, 32, 33
**Produces:** the four permission modes with the `unrestricted` default, three file-backed scopes plus the legacy XDG path, and shell-analysis bypass rejection
**Pointers:** `crates/lotta-tools/src/permissions/mode.rs`, `permissions/scopes.rs`, `permissions/matcher.rs`, `permissions/analyzer.rs`; reference: `../letta-code/src/permissions/mode.ts`, `../letta-code/src/permissions/loader.ts`, `../letta-code/src/permissions/matcher.ts`, `../letta-code/src/permissions/checker.ts`, `../letta-code/src/permissions/analyzer.ts`

## Steps

- [ ] Implement modes `standard`, `acceptEdits`, `unrestricted`, and `strict`, defaulting to `unrestricted` as the pinned baseline does
- [ ] Load file-backed rules from user (`~/.letta/settings.json` plus the legacy XDG path), project (`.letta/settings.json`), and local (`.letta/settings.local.json`) scopes, and confirm there is no baseline agent-file permission scope
- [ ] Layer session and mod rules in at check time
- [ ] Match on normalized tool name, command, path, cwd, runtime scope, and requested action
- [ ] Canonicalize file and memory paths before policy and reject shell bypasses by analysis rather than string prefixes
- [ ] Cover symlink traversal, `..`, alternate separators, shell redirection, subprocess launch, and command substitution with negative tests

## Definition of done

- [ ] All four permission modes exist with `unrestricted` as the default, and each mode's decision differs where the baseline differs
- [ ] Rules load from the three file-backed scopes plus the legacy XDG path, with session and mod rules layered at check time and no agent-file scope
- [ ] Matching operates on normalized tool name, command, path, cwd, runtime scope, and requested action, with paths canonicalized before policy
- [ ] Shell analysis rejects bypasses rather than relying on string prefixes, with negative tests for symlink traversal, `..`, alternate separators, redirection, subprocess launch, and command substitution
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(permissions::)'` and sees four modes with the `unrestricted` default, four scope loaders, six matching inputs, and six bypass rejections pass
