# Task 37 — File built-in tools

**Plan:** [plan.md](../plan.md) · **Certificate:** [37-builtin_file_tools-certificate.md](37-builtin_file_tools-certificate.md)

**Implements:** [05-tools-and-extensions.md §Rust built-ins](../../../05-tools-and-extensions.md#rust-built-ins)
**Depends on:** 33, 34, 35
**Produces:** the ten file built-ins — read, write, edit, multi-edit, apply-patch, list, glob, grep, image view, and artifact-file read/write — under policy, sandbox, and clamps
**Pointers:** `crates/lotta-tools/src/builtin/file/`; reference: `../letta-code/src/tools/impl/read.ts`, `edit.ts`, `multi-edit.ts`, `apply-patch.ts`, `ls.ts`, `glob.ts`, `grep.ts`, `view-image.ts`, `artifact-files.ts`, `write.ts`

## Steps

- [x] Implement read, write, edit, multi-edit, and apply-patch with canonicalized paths and confinement
- [x] Implement list, glob, and grep with bounded result sets and the grep-family 10,000-character clamp
- [x] Implement image view and artifact-file read/write with `IMAGE_BYTES_MAX` enforcement
- [x] Register each tool with its per-toolset model-facing names, including the Codex and Gemini variants
- [x] Classify every file tool's parallel safety explicitly, defaulting to sequential
- [x] Add negative tests for traversal, symlinks, oversized inputs, and out-of-root writes

## Definition of done

- [x] All ten file built-ins named in `05-tools-and-extensions.md` §Rust built-ins exist and execute through the Task 33 pipeline
- [x] Each tool's per-toolset model-facing names match the baseline, including Codex and Gemini variants
- [x] Path traversal, symlink escape, and out-of-root write are rejected for every path-taking file tool
- [x] Grep clamps at 10,000 model-facing characters and the other families at 30,000/32,000, with overflow written to a file
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(builtin::file::)'` and sees ten tools execute, baseline names match, confinement reject traversal and symlinks, and family clamps apply
