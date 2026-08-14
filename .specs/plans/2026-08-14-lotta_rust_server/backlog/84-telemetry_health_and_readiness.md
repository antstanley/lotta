# Task 84 — Telemetry, security logging, health, and readiness

**Plan:** [plan.md](../plan.md) · **Certificate:** [84-telemetry_health_and_readiness-certificate.md](84-telemetry_health_and_readiness-certificate.md)

**Implements:** [07-channels-and-operations.md §Observability and security](../../../07-channels-and-operations.md#observability-and-security) · [07-channels-and-operations.md §Health and readiness](../../../07-channels-and-operations.md#health-and-readiness) · [architecture-principles.md §Secrets](../../../architecture-principles.md#secrets)
**Depends on:** 05, 22, 83
**Produces:** structured JSON daemon logs with the required fields and centralized scrubbing, plus the four health, readiness, capability, and metrics endpoints
**Pointers:** `crates/lotta-telemetry/src/logging.rs`, `src/scrub.rs`, `src/metrics.rs`, `crates/lotta-app-server/src/health.rs`; reference: `../letta-code/src/websocket/listener/protocol-logging.ts`, `../letta-code/src/websocket/listener/commands/app-server-info.ts`

## Steps

- [ ] Emit structured JSON logs in daemon mode carrying timestamp, level, subsystem, incident ID, runtime key, connection ID, turn ID, and stable error code where applicable
- [ ] Exclude user content and secrets by default and pass every log and error through centralized scrubbing
- [ ] Serve `GET /healthz` as baseline-compatible liveness
- [ ] Serve `GET /readyz` with Lotta's stronger readiness: state validated, auth loaded, runtime registry accepting work
- [ ] Serve `GET /app-server-info` as an authenticated HTTP capability snapshot matching the WebSocket command, and `GET /metrics` only when enabled and separately authenticated or bound
- [ ] Keep `/ws` and the OpenAI routes not-ready until storage validation completes, while provider outages never fail readiness

## Definition of done

- [ ] Daemon logs are structured JSON carrying all eight required fields where applicable, with user content and secrets excluded
- [ ] `/healthz` is liveness only while `/readyz` requires state validation, loaded auth, and an accepting runtime registry
- [ ] `/ws` and the OpenAI routes are not ready until storage validation completes, and a provider outage never changes readiness
- [ ] `/app-server-info` requires authentication and matches the WebSocket capability snapshot; `/metrics` is served only when enabled and separately authenticated or bound
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer starts the server, curls `/healthz` and `/readyz` before and after storage validation, and curls `/app-server-info` with and without credentials, seeing liveness always healthy, readiness gated, and the capability route authenticated
