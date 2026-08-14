# Task 30 — System prompt compilation and cache reuse

**Plan:** [plan.md](../plan.md) · **Certificate:** [30-memfs_prompt_compilation-certificate.md](30-memfs_prompt_compilation-certificate.md)

**Implements:** [04-persistence-and-memfs.md §Prompt compilation](../../../04-persistence-and-memfs.md#prompt-compilation)
**Depends on:** 03, 29
**Produces:** `system-prompt.json` with exactly the six compatibility fields and cache reuse keyed on `rawSystemHash` and `memfsRevision`
**Pointers:** `crates/lotta-memfs/src/prompt/compile.rs`, `prompt/cache.rs`, `prompt/record.rs`; reference: `../letta-code/src/backend/local/system-prompt-compilation.ts`, `../letta-code/src/websocket/listener/memfs-sync.ts`

## Steps

- [ ] Compile from managed/custom system text, committed memory files and tree, selected skills, runtime reminders, and tool/model guidance
- [ ] Persist exactly `content`, `coreMemory`, optional `midConversationSystemPrompt`, `compiledAt`, `rawSystemHash`, and optional `memfsRevision` — and nothing else
- [ ] Reuse the cache by comparing `rawSystemHash` and `memfsRevision` only
- [ ] Detect memory changes by committed revision, leaving an uncommitted working tree visible to tools but out of the authoritative prompt
- [ ] Inject a committed memory update mid-conversation for providers that support system messages, otherwise apply the new prompt at the next provider request boundary
- [ ] Round-trip a compiled record against the `fixtures/persistence/` prompt fixture

## Definition of done

- [ ] `system-prompt.json` contains exactly the six fields of §Prompt compilation and no others
- [ ] Cache reuse compares only `rawSystemHash` and `memfsRevision`; an unchanged pair skips recompilation and a changed one forces it
- [ ] An uncommitted memory working tree is visible to tools but does not become authoritative prompt memory until committed
- [ ] A committed memory update is injected mid-conversation where the provider supports system messages, and otherwise applies at the next provider request boundary
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-memfs -E 'test(prompt::)'` and sees the exact six-field record, cache reuse on both keys, uncommitted-not-authoritative, and both mid-conversation paths pass
