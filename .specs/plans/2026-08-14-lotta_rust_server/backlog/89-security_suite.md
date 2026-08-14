# Task 89 — Security suite

**Plan:** [plan.md](../plan.md) · **Certificate:** [89-security_suite-certificate.md](89-security_suite-certificate.md)

**Implements:** [00-overview.md §Implementation acceptance](../../../00-overview.md#implementation-acceptance) · [07-channels-and-operations.md §Observability and security](../../../07-channels-and-operations.md#observability-and-security) · [architecture-principles.md §Secrets](../../../architecture-principles.md#secrets)
**Depends on:** 14, 34, 35, 52, 84, 85
**Produces:** proof of non-loopback authentication, Origin rejection, path confinement, secret redaction, and sandbox policy against the assembled binary
**Pointers:** `tests/conformance/security.rs`; reference: `../letta-code/src/websocket/app-server-auth.ts`, `../letta-code/src/websocket/app-server.ts`, `../letta-code/src/permissions/analyzer.ts`, `../letta-code/src/sandbox/policy.ts`

## Steps

- [ ] Assert a non-loopback bind without authentication fails before listening, and that both auth modes accept valid and reject invalid credentials
- [ ] Assert an unauthenticated Origin-bearing client is rejected even on loopback
- [ ] Assert signed-bearer rejection for a missing `exp`, a wrong issuer, a wrong audience, a non-HS256 algorithm, and a skew beyond the configured bound
- [ ] Assert path confinement across file tools, memory tools, and the files command group, covering `..`, symlinks, alternate separators, and absolute paths
- [ ] Assert secret redaction across logs, errors, App Server snapshots, and channel config responses using planted markers
- [ ] Assert the sandbox blocks out-of-root reads and that an unsupported platform errors rather than running unsandboxed

## Definition of done

- [ ] Non-loopback binding without authentication fails before listening, and both auth modes accept valid and reject invalid credentials
- [ ] An unauthenticated Origin-bearing client is rejected even on loopback, and every signed-bearer rejection class is covered
- [ ] Path confinement holds across file tools, memory tools, and the files command group for all four escape classes
- [ ] Planted secret markers are absent from logs, errors, App Server snapshots, and channel config responses
- [ ] The sandbox blocks out-of-root reads and an unsupported platform errors rather than running unsandboxed
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `cargo nextest run --test security` against the assembled binary and sees auth, Origin, signed-bearer, confinement, redaction, and sandbox cases pass
