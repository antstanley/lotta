# Task 83 — Configuration and CLI surface

**Plan:** [plan.md](../plan.md) · **Certificate:** [83-configuration_and_cli_surface-certificate.md](83-configuration_and_cli_surface-certificate.md)

**Implements:** [02-app-server-api.md §Listener configuration](../../../02-app-server-api.md#listener-configuration) · [architecture-principles.md §Cross-cutting conventions](../../../architecture-principles.md#cross-cutting-conventions) · [00-overview.md §Scope summary](../../../00-overview.md#scope-summary)
**Depends on:** 05, 14, 15
**Produces:** the `lotta server` and `lotta local-backend` CLI surface with validated configuration and secret references, and no configuration format beyond what the spec authorizes
**Pointers:** `crates/lotta-config/src/cli.rs`, `src/validate.rs`, `src/secret_ref.rs`; reference: `../letta-code/src/cli/subcommands/listen.tsx`, `../letta-code/src/websocket/app-server-auth.ts`, `../letta-code/src/settings.ts`

## Steps

- [ ] Implement `lotta server --backend local --listen [...]` with the nine listener flags and `--openai-api`, `--no-mods`
- [ ] Implement `lotta local-backend migrate-transcripts` and `lotta local-backend verify` as the CLI entry points for Task 26
- [ ] Resolve `LETTA_LOCAL_BACKEND_DIR` and `LETTA_HOME` with the documented defaults
- [ ] Represent secret material as file references resolved once at startup into `Secret` values, never as inline configuration strings
- [ ] Validate every configuration value before the listener binds, failing with a message naming the offending flag
- [ ] Print the resolved base URL, WebSocket URL, and optional OpenAI base URL at startup

## Definition of done

- [ ] The CLI exposes exactly the flags `02-app-server-api.md` §Listener configuration names, plus `--openai-api` and `--no-mods`, and rejects an unknown flag
- [ ] `LETTA_LOCAL_BACKEND_DIR` and `LETTA_HOME` resolve with the documented defaults and overrides
- [ ] Secret material is supplied only by absolute file reference, resolved once at startup into a `Secret`, with no inline secret flag
- [ ] Configuration is validated before the listener binds, and a failure names the offending flag
- [ ] Meets the repo definition of done (tests, lint/format, named-constant limits — see plan.md baseline)
- [ ] Reviewable: a reviewer runs `lotta server --help`, then `lotta server --backend local --listen --ws-auth capability-token` without a token file, and sees the documented flag list and a startup failure naming the missing flag before any socket opens

## Open questions

- Should Lotta gain a configuration-file format and environment-variable prefix beyond the documented CLI flags and `LETTA_HOME`/`LETTA_LOCAL_BACKEND_DIR`? No canonical page defines one today, so this task ships flags only.
