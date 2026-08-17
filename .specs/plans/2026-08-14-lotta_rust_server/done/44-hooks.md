# Task 44 — Hook events, command and prompt executors, and owner attribution

**Plan:** [plan.md](../plan.md) · **Certificate:** [44-hooks-certificate.md](44-hooks-certificate.md)

**Implements:** [05-tools-and-extensions.md §Hooks and mods](../../../05-tools-and-extensions.md#hooks-and-mods)
**Depends on:** 33, 35
**Produces:** the eleven hook events with the prompt-hook subset restriction, sandboxed command hooks, and per-owner failure attribution
**Pointers:** `crates/lotta-extensions/src/hooks/events.rs`, `hooks/command.rs`, `hooks/prompt.rs`, `hooks/loader.rs`; reference: `../letta-code/src/hooks/types.ts`, `loader.ts`, `executor.ts`, `prompt-executor.ts`

## Steps

- [x] Define the hook event set from `05-tools-and-extensions.md` §Hooks and mods: pre-tool, post-tool, tool failure, permission request, user prompt, notification, stop, subagent stop, pre-compact, session start, session end
- [x] Restrict prompt hooks to pre-tool, post-tool, tool failure, permission request, user prompt, stop, and subagent stop; keep notification, pre-compact, and session events command-hook-only
- [x] Run command hooks as sandboxed child processes under `COMMAND_HOOK_TIMEOUT_MS_DEFAULT`
- [x] Run prompt hooks under `PROMPT_HOOK_TIMEOUT_MS_DEFAULT`
- [x] Implement block, modify, and allow outcomes and attribute a failure to its owning hook
- [x] Enforce `HOOKS_PER_EVENT_MAX` and fire hooks in load order

## Definition of done

- [x] All eleven hook events exist with the spec's names and each fires at its documented point
- [x] Prompt hooks are supported for exactly the seven permitted events, and registering a prompt hook for notification, pre-compact, or a session event is rejected
- [x] Command hooks run as sandboxed child processes under `COMMAND_HOOK_TIMEOUT_MS_DEFAULT`, and prompt hooks under `PROMPT_HOOK_TIMEOUT_MS_DEFAULT`
- [x] Block, modify, and allow outcomes work, a failure is attributed to its owning hook, and `HOOKS_PER_EVENT_MAX` rejects at the limit
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(hooks::)'` and sees eleven events, the seven-event prompt subset with four rejections, both timeouts, sandboxed command hooks, and owner attribution pass
