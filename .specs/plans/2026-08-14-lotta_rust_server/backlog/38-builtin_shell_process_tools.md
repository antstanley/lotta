# Task 38 — Shell and process built-in tools

**Plan:** [plan.md](../plan.md) · **Certificate:** [38-builtin_shell_process_tools-certificate.md](38-builtin_shell_process_tools-certificate.md)

**Implements:** [05-tools-and-extensions.md §Rust built-ins](../../../05-tools-and-extensions.md#rust-built-ins)
**Depends on:** 33, 34, 35
**Produces:** one-shot shell/exec, PTY session, background output, stdin, monitor, stop, and timeout under sandbox with the baseline output bounds
**Pointers:** `crates/lotta-tools/src/builtin/shell/`; reference: `../letta-code/src/tools/impl/shell.ts`, `bash.ts`, `exec-command.ts`, `shell-runner.ts`, `shell-launchers.ts`, `bash-output.ts`, `kill-bash.ts`, `monitor.ts`, `process_manager.ts`, `shell-env.ts`

## Steps

- [ ] Implement one-shot shell/exec and PTY/session execution through the sandbox port
- [ ] Implement background output retrieval, stdin write, monitor, and stop
- [ ] Apply `LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT` with bounded per-tool overrides, keeping timeout distinct from cancellation
- [ ] Bound captured output at `CHILD_PROCESS_OUTPUT_BYTES_MAX` and clamp the model-facing result at the shell-family 30,000 characters
- [ ] Terminate children with SIGTERM then SIGKILL after a 2,000 ms grace
- [ ] Register per-toolset names including the Codex and Gemini shell variants

## Definition of done

- [ ] The seven shell/process behaviors of §Rust built-ins execute: one-shot shell/exec, PTY session, background output, stdin, monitor, stop, and timeout
- [ ] Timeout is distinct from cancellation and both are bounded: `LOCAL_TOOL_EXECUTION_TIMEOUT_MS_DEFAULT` interrupts, and a per-tool override stays bounded
- [ ] Child termination is two-stage: SIGTERM then SIGKILL after a 2,000 ms grace, and no orphan survives
- [ ] Captured output is bounded at `CHILD_PROCESS_OUTPUT_BYTES_MAX` and the model-facing result clamps at the shell-family 30,000 characters with overflow written to a file
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run -p lotta-tools -E 'test(builtin::shell::)'` and sees the seven behaviors, distinct timeout and cancellation outcomes, two-stage kill, and both output bounds pass
