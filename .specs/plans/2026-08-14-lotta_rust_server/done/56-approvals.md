# Task 56 — Approvals, resolution validation, and recovery

**Plan:** [plan.md](../plan.md) · **Certificate:** [56-approvals-certificate.md](56-approvals-certificate.md)

**Implements:** [03-runtime-and-turns.md §Approvals](../../../03-runtime-and-turns.md#approvals) · [01-domain-model.md §Approval](../../../01-domain-model.md#approval)
**Depends on:** 55
**Produces:** approval requests stored before emission, resolution validated against the original schema, and a 24-hour interruption that never becomes an implicit denial
**Pointers:** `crates/lotta-runtime/src/approval/request.rs`, `approval/resolve.rs`, `approval/recovery.rs`; reference: `../letta-code/src/websocket/listener/approval.ts`, `turn-approval.ts`, `approval-suggestions.ts`, `auth-lifecycle-approval-reconnect.test.ts`

## Steps

- [x] Store the approval request before emitting `control_request`, keeping it part of the active lease
- [x] Validate resolution against request ID, tool call ID, lease generation, and edited input against the original tool schema
- [x] Execute an allowed call exactly once; append a structured denied tool result and resume the model where the tool protocol requires continuation
- [x] Move `Active` to `Cancelling` on abort and make a late approval response unable to clear cancellation
- [x] On `APPROVAL_WAIT_MS_MAX`, interrupt the turn and replay an explicit expired approval state rather than denying
- [x] Replay unresolved requests on reconnect `sync`, and on process restart either reconstruct a safe continuation or emit explicit denial/interruption without guessing that a tool ran; enforce `PENDING_APPROVALS_PER_RUNTIME_MAX`

## Definition of done

- [x] The request is stored before `control_request` is emitted, and resolution validates request ID, tool call ID, lease generation, and edited input against the original schema
- [x] An approval timeout at `APPROVAL_WAIT_MS_MAX` interrupts the turn and replays an explicit expired approval state; it never appends a denial
- [x] Allow executes the call exactly once; deny appends a structured denied result and resumes the model where the protocol requires continuation; abort moves to `Cancelling` and a late response cannot clear it
- [x] Reconnect `sync` replays unresolved requests, and restart recovery never guesses that a tool ran
- [x] `PENDING_APPROVALS_PER_RUNTIME_MAX` fails the turn as an invariant violation at the limit
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(approval::)'` and sees store-before-emit, four resolution validations, interruption-not-denial on timeout, the three resolutions, and both recovery paths pass

## Open questions

- Is a 24-hour approval expiry acceptable for all Desktop and channel workflows, or should the bound be configurable below a larger hard maximum (`03-runtime-and-turns.md` §Open questions, *Approval compatibility*)?
