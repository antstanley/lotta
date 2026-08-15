# Task 29 — MemFS Git repositories and path normalization

**Plan:** [plan.md](../plan.md) · **Certificate:** [29-memfs_git_repositories-certificate.md](29-memfs_git_repositories-certificate.md)

**Implements:** [04-persistence-and-memfs.md §MemFS](../../../04-persistence-and-memfs.md#memfs)
**Depends on:** 06, 09, 22
**Produces:** one Git repository per agent under the backend root with the baseline label normalization and the full operation set
**Pointers:** `crates/lotta-memfs/src/repo.rs`, `src/labels.rs`, `src/ops.rs`, `src/worktree.rs`; reference: `../letta-code/src/agent/memory-filesystem.ts`, `../letta-code/src/agent/memory-git.ts`

## Steps

- [x] Initialize an independent Git repository at `memfs/<agent-id>/memory/` with the `system/`, `skills/`, and `mods/` layout
- [x] Render memory blocks to `system/<normalized-label>.md`, keeping labels already beginning with `system/` in place
- [x] Normalize backslashes and a trailing `.md`, collapse empty/absolute-looking labels through the baseline segment normalization, and reject `.` and `..` segments
- [x] Fall back to `Memory block <label>` for a missing description so every generated file has non-empty YAML frontmatter
- [x] Implement status, tree, read, write, delete, rename, history, file-at-revision, diff, commit, worktree-based reflection/merge, pre-commit Markdown validation, and optional post-commit push to `letta.memoryRepository.url`
- [x] Enforce `MEMORY_FILE_BYTES_MAX` and `MEMORY_FILES_MAX`, and confirm no implicit Letta remote is configured

## Definition of done

- [x] Label normalization matches the baseline: `system/` prefixes are preserved, backslashes and trailing `.md` are normalized, empty/absolute labels collapse, and `.`/`..` segments are rejected
- [x] Every generated file carries non-empty YAML frontmatter, with a missing description defaulting to `Memory block <label>`
- [x] The full operation set works: status, tree, read, write, delete, rename, history, file-at-revision, diff, commit, worktree reflection/merge, pre-commit validation, and optional post-commit push
- [x] No implicit Letta remote is configured, and `MEMORY_FILE_BYTES_MAX`/`MEMORY_FILES_MAX` reject at the limit
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-memfs -E 'test(labels::) + test(ops::) + test(repo::)'` and sees normalization rules, the thirteen operations, frontmatter fallback, no implicit remote, and both bounds pass
