# Task 06 — Core port traits and dependency direction

**Plan:** [plan.md](../plan.md) · **Certificate:** [06-core_port_traits-certificate.md](06-core_port_traits-certificate.md)

**Implements:** [architecture-principles.md §Architectural pattern](../../../architecture-principles.md#architectural-pattern) · [architecture-principles.md §What goes where](../../../architecture-principles.md#what-goes-where) · [architecture-principles.md §Dependency graph](../../../architecture-principles.md#dependency-graph) · [architecture-principles.md §Concurrency](../../../architecture-principles.md#concurrency) · [04-persistence-and-memfs.md §Responsibilities](../../../04-persistence-and-memfs.md#responsibilities)
**Depends on:** 04, 05
**Produces:** the port traits every adapter implements, defined in `lotta-runtime` so the runtime never links a concrete adapter crate
**Pointers:** `crates/lotta-runtime/src/ports/mod.rs`, `ports/clock.rs`, `ports/ids.rs`, `ports/store.rs`, `ports/transcript.rs`, `ports/memfs.rs`, `ports/sandbox.rs`, `ports/child_process.rs`; reference: `../letta-code/src/backend/backend.ts`, `../letta-code/src/backend/local/local-backend.ts`

## Steps

- [x] Create `lotta-runtime::ports` holding `IdGenerator`, `AgentStore`, `ConversationStore`, `TranscriptStore`, `MemFsPort`, `SandboxPort`, and `ChildProcessPort`, and re-export the already-canonical `lotta_domain::Clock` time port from that module rather than defining a duplicate trait
- [x] Express filesystem, process, clock, random, and Git effects as core port methods now; keep network/provider effects absent from runtime implementation and reserve their typed `ProviderPort` boundary for Task 07
- [x] Document on each trait its preconditions, error type, cancellation behavior, and ownership, per `development-guidelines.md` §Documentation
- [x] Add a workspace dependency assertion that `lotta-runtime` declares no dependency on `lotta-store`, `lotta-memfs`, `lotta-providers`, `lotta-tools`, `lotta-extensions`, `lotta-channels`, or `lotta-app-server`
- [x] Use bounded channels and explicit cancellation tokens in every port signature that streams
- [x] Record the port-placement decision in the crate root doc comment so adapters know they depend on runtime interfaces, never the reverse

## Definition of done

- [x] Every Task-06-owned effect named in `architecture-principles.md` §Architectural pattern (filesystem, process, clock, random, Git) is expressed as a port trait method or canonical re-export in `lotta-runtime::ports`; network/provider effects remain absent pending Task 07's typed provider boundary
- [x] `lotta-runtime` declares no dependency on any adapter crate, and the assertion is enforced by a test rather than by review
- [x] Every streaming port method carries an explicit cancellation token and a bounded channel; no unbounded channel appears in a port signature
- [x] Each trait's rustdoc states its preconditions, error type, cancellation behavior, and ownership
- [x] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [x] Reviewable: a reviewer runs `cargo nextest run -p lotta-runtime -E 'test(ports::)'` and sees the dependency-direction assertion and port-signature checks pass, then confirms `cargo tree -p lotta-runtime` lists no adapter crate
