# Task 40 — Planning, task, skill-load, interaction, and LSP built-in tools

**Plan:** [plan.md](../plan.md) · **Certificate:** [40-builtin_planning_task_interaction_lsp_tools-certificate.md](40-builtin_planning_task_interaction_lsp_tools-certificate.md)

**Implements:** [05-tools-and-extensions.md §Rust built-ins](../../../05-tools-and-extensions.md#rust-built-ins)
**Depends on:** 33, 34, 36
**Produces:** `update_plan`/`UpdatePlan`, `TodoWrite`, the six task lifecycle tools, the skill loader, the approval and ask-user bridge, and `ReadLSP` diagnostics
**Pointers:** `crates/lotta-tools/src/builtin/planning/`, `builtin/task/`, `builtin/skill.rs`, `builtin/interaction.rs`, `builtin/lsp.rs`; reference: `../letta-code/src/tools/impl/update-plan.ts`, `todo-write.ts`, `task-create.ts`, `task-get.ts`, `task-list.ts`, `task-update.ts`, `task-output.ts`, `task-stop.ts`, `skill.ts`, `ask-user-question.ts`, `read-lsp.ts`

## Steps

- [x] Implement `update_plan` and its `UpdatePlan` alias over one internal implementation, plus `TodoWrite`
- [x] Implement task create, get, list, update, output, and stop
- [x] Implement the skill loader that reads one registered skill and its companion files through Task 36
- [x] Implement the interaction bridge for approval and ask-user-question
- [x] Implement `ReadLSP` diagnostics through configured language servers, with no definition or reference lookup beyond what §Rust built-ins names
- [x] Register per-toolset names and classify each tool's parallel safety

## Definition of done

- [x] The planning and task surface is exactly `update_plan`/`UpdatePlan`, `TodoWrite`, and the six task lifecycle tools — no invented plan or memory-recall tool
- [x] The skill-load tool reads one registered skill and its companion files, and fails for an unregistered skill
- [x] The interaction bridge produces approval and ask-user-question requests that pause the caller and resume on response
- [x] `ReadLSP` returns diagnostics only, with no definition or reference lookup
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(builtin::planning::) + test(builtin::task::) + test(builtin::skill::) + test(builtin::interaction::) + test(builtin::lsp::)'` and sees the exact baseline tool set with no invented names
