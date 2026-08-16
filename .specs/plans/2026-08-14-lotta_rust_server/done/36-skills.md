# Task 36 — Skill discovery precedence, frontmatter fallbacks, and selection

**Plan:** [plan.md](../plan.md) · **Certificate:** [36-skills-certificate.md](36-skills-certificate.md)

**Implements:** [05-tools-and-extensions.md §Skills](../../../05-tools-and-extensions.md#skills)
**Depends on:** 27, 29, 30
**Produces:** skill discovery in the exact four-level precedence with the baseline optional-frontmatter fallbacks and runtime source restriction
**Pointers:** `crates/lotta-extensions/src/skills/discovery.rs`, `skills/frontmatter.rs`, `skills/load.rs`; reference: `../letta-code/src/agent/skills.ts`, `../letta-code/src/skills/builtin`, `../letta-code/src/websocket/listener/skill-injection.ts`, `../letta-code/src/tools/impl/skill-content-registry.ts`

## Steps

- [ ] Discover in precedence order: project (`.agents/skills`, legacy `.skills` fallback), agent (`~/.letta/agents/<agent-id>/memory/skills`, `$MEMORY_DIR/skills` read fallback), global (`~/.letta/skills`), then bundled
- [ ] Treat frontmatter `id`, `name`, and `description` as optional, deriving ID and name from the path and falling back to the first body paragraph then `No description available`
- [ ] Let runtime selection restrict sources without changing the precedence among the sources that remain
- [ ] Load a skill's complete instructions and companion files on demand
- [ ] Run skill scripts under the same permission and sandbox policy as direct tools
- [ ] Feed the selected skill set into prompt compilation

## Definition of done

- [ ] Discovery precedence is project, then agent, then global, then bundled, with both legacy fallback paths honoured
- [ ] Optional frontmatter falls back exactly as the baseline does: ID and name from the path, description from the first body paragraph, then `No description available`
- [ ] Runtime selection restricts sources without reordering the remaining precedence
- [ ] Loading a skill reads its complete instructions and companion files, and skill scripts run under the same permission and sandbox policy as direct tools
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-extensions -E 'test(skills::)'` and sees project-over-agent precedence, both legacy fallbacks, the three frontmatter fallbacks, source restriction, and script policy pass
