# 07 — Channels and Operations

**Status:** Draft · **Date:** 2026-08-14 · **Owner:** Ant Stanley · **Scope:** Repo-wide

This page defines messaging-channel integration and self-hosted deployment. Channel execution remains a separate supervised process: App Server WebSocket carries runtime data and a private stdin/stdout control plane carries management, matching the pinned topology.

---

## Responsibilities

1. Run the App Server as an always-on local service with persistent state.
2. Supervise configured messaging adapters in a separate channel host child process.
3. Preserve account, pairing, route, target, access-control, progress, and reply behavior.
4. Restore enabled channels after restart without bypassing authentication or runtime setup.
5. Provide health, readiness, metrics, logs, backup, upgrade, and graceful shutdown contracts.

---

## Process topology

```text
 Telegram  Slack  Discord  Signal  WhatsApp  Custom
     \       |       |       |       |       /
      └──────┴───────┴───────┴───────┴──────┘
                     │ platform APIs
                     ▼
          lotta-channel-host (Bun/Node compatibility)
          ├── plugin registry + accounts
          ├── pairing, routing, access control
          ├── media, typing, progress rendering
          └── MessageChannel executor
                  │                 │
       authenticated loopback WS    │ newline-delimited JSON control
                  │                 │ over supervised stdin/stdout
                  ▼                 ▼
              lotta-server /ws + channel supervisor
          ├── runtime_start + external tools
          ├── input admission and stream events
          ├── channel management/tool publication control
          └── shared local agent state
```

The channel host uses the public App Server protocol for runtime data: it registers `MessageChannel`, starts routed runtimes, submits input, and consumes stream/state events. The supervising server also uses a newline-delimited JSON control plane for channel management, `publish_runtime_tools`, `release_runtime_tools`, and the `/channels` slash command. The host does not import Rust internals or write the local-backend store directly; it owns channel files below `~/.letta/channels`.

---

## Account and routing model

A channel plugin exposes metadata, account configuration schema, adapter lifecycle, and message actions. First-party channel IDs are `telegram`, `slack`, `discord`, `custom`, `whatsapp`, and `signal`; `custom` is the built-in loader for user-defined channel plugins. Custom plugin directories under `~/.letta/channels/<channel-id>/` contain `channel.json`, an entry module, accounts, routing, pairing, and runtime dependencies.

Inbound flow:

1. Adapter normalizes the platform message and sender identity.
2. Access control evaluates global/per-channel allowlists, DM/group policy, paired users, and admin command tier.
3. Slash commands execute before ordinary agent ingress.
4. Routing resolves account/chat/thread to agent/conversation or creates a bounded pairing code.
5. ChannelGateway performs `runtime_start`, publishes `MessageChannel`, and submits `input` with stable `client_message_id`.
6. Lifecycle/progress events update typing, draft, status, and error presentation.
7. The agent calls `MessageChannel` to send a visible reply through the originating adapter.

Inbound delivery and outbound reply remain separate. A turn can complete without a channel reply if the model does not call the tool.

---

## Access control

Every inbound message, including groups and auto-routed Slack/Discord traffic, passes central sender gating before commands or routes.

Account policy includes:

- `dm_policy`: `pairing`, `allowlist`, or `open`
- `group_policy`: `open` or `allowlist`
- `allowed_users`
- `admin_users`
- `user_allowed_commands`

Environment allowlists and explicit allow-all flags merge according to baseline behavior. Pairing approval adds permission; it never removes a stricter global deny. Secret config values are redacted from App Server snapshots.

Pairing codes use cryptographic randomness, expire after 15 minutes, and are single-use. A sender/account pair reuses its one unexpired code instead of minting another. Each channel store keeps at most 50 pending codes and evicts the oldest after pruning expired entries; the baseline has no source-address rate limiter.

---

## Channel command surface

Shared operational commands are `help`, `status`, `whoami`, `pause`, `resume`, `cancel`, `chat`, `feedback`, `model`, `reflection` (alias `reflect`), and `reload`. Command authorization is tiered separately from message admission. Unsupported commands return help rather than entering the agent transcript.

The channel management protocol has exactly 20 commands: `channels_list`, `channel_accounts_list`, account create/update/bind/unbind/delete/start/stop, config get/set, channel start/stop, pairing list/bind, route list/update/remove, and target list/bind. `channels_updated`, `channel_accounts_updated`, `channel_pairings_updated`, and `channel_targets_updated` are push events, not commands.

---

## Deployment image

The self-hosted Lotta image contains:

- the statically linked `lotta` binary
- Git, CA certificates, and optional OS sandbox runtime
- Bun/Node only when JS compatibility hosts or channels are enabled
- cron compatibility only when OS cron tasks are explicitly configured; agent schedules run in Lotta

Default command:

```text
lotta server --backend local \
  --listen ws://0.0.0.0:4500 \
  --ws-auth capability-token \
  --ws-token-file /run/secrets/lotta-app-server-token
```

The deployment mounts one persistent state volume and read-only secret files:

```text
/home/lotta/.letta          -> full $HOME/.letta state volume; LETTA_HOME points here
/home/lotta/.letta/lc-local-backend -> LETTA_LOCAL_BACKEND_DIR
/run/secrets/*                -> App Server auth and backup-encryption material
/workspace                    -> optional tool workspace
```

Lotta supervises the channel host as a child process in the same service/container or pod, using the same persistent channel configuration and a loopback-scoped App Server token. An independently deployed host would lose the stdin/stdout management plane and is not the initial parity topology.

---

## Health and readiness

| Endpoint | Meaning |
|---|---|
| `GET /healthz` | Baseline-compatible liveness path; process event loop and acceptor are alive |
| `GET /readyz` | Baseline-compatible path with Lotta's stronger readiness semantics: state validated, auth loaded, runtime registry accepting work |
| `GET /app-server-info` | Authenticated HTTP capability snapshot matching WebSocket discovery |
| `GET /metrics` | Lotta-additive Prometheus metrics when enabled and separately authenticated/bound |

OpenAI routes and `/ws` are not considered ready until storage validation completes. The baseline returns `ok` from both probe paths without these stronger checks; Lotta's readiness meaning is explicit hardening, while the route names remain compatible. Provider outages do not fail process readiness; affected model status reports unavailable.

---

## Shutdown and restart

SIGTERM/SIGINT executes:

1. mark not ready and stop new upgrades/admissions,
2. notify clients of shutdown status,
3. request cancellation for active turns,
4. stop scheduler admissions and channel delivery,
5. wait bounded grace for turns and persistence,
6. kill scoped child processes and compatibility hosts,
7. flush metrics/logs and release store lock,
8. close sockets and exit.

Restart reloads agents lazily, validates transcript manifests, restores schedules, recompiles prompts only when inputs changed, and leaves channels to the separate channel host.

---

## Observability and security

Logs are structured JSON in daemon mode. Required fields include timestamp, level, subsystem, incident ID, runtime key, connection ID, turn ID, and stable error code where applicable. User content and secrets are excluded by default.

The server runs as a non-root user, uses a read-only root filesystem, drops Linux capabilities, and grants write access only to state/workspace/temp paths. Non-loopback traffic requires authentication and deployment TLS. App Server tokens never ship to browser code; browser applications use a trusted backend.

---

## Operational bounds

Pairing values match the baseline. Remaining resource and restart bounds are Lotta hardening unless a reference fixture establishes the same value.

| Constant | Default |
|---|---:|
| `CHANNEL_ACCOUNTS_MAX` | 256 |
| `CHANNEL_ROUTES_MAX` | 100,000 |
| `PAIRINGS_PENDING_PER_CHANNEL_MAX` | 50 |
| `PAIRING_TTL_SECONDS` | 900 |
| `CHANNEL_MESSAGE_BYTES_MAX` | 16 MiB |
| `CHANNEL_MEDIA_BYTES_MAX` | 50 MiB |
| `CHANNEL_DELIVERY_RETRIES_MAX` | 5 |
| `SHUTDOWN_GRACE_MS` | 30,000 |
| `SIDECAR_RESTARTS_PER_HOUR_MAX` | 10 (Lotta hardening) |

---

## Compatibility with letta-app-server-deployment

The adjacent deployment repository currently starts Cloud remote-environment mode and does not expose a port. A Lotta deployment variant changes that topology:

| Current deployment | Lotta local deployment |
|---|---|
| `letta server --env-name ...` | `lotta server --backend local --listen ...` |
| outbound Cloud WebSocket | inbound authenticated App Server WebSocket |
| `/root` persistent volume | non-root full `~/.letta` state volume, including backend and side stores |
| OAuth/API-key registration | local capability/JWT authentication |
| server restores channels through a supervised child | Lotta restores channels through its supervised child |
| no exposed application port | authenticated port 4500 behind TLS/private network |

The existing published Letta Code image remains a separate deployment target; Lotta does not silently replace it.

---

## Assumptions and open questions

**Assumptions**

- JavaScript channel SDK dependencies remain available to the compatibility host.
- Operators provide TLS or private networking around a non-loopback App Server.

**Decisions**

- *Channel topology.* **Separate supervised process with two bounded interfaces.** App Server WebSocket carries runtime data; newline-delimited JSON over stdin/stdout carries management and runtime-tool publication.
- *Channel state ownership.* **The host owns `~/.letta/channels`; Lotta owns the backend root.** Backup covers both without allowing the channel process to write agent transcripts.
- *Container user.* **Non-root.** Tool access reaches sensitive host resources, but the daemon itself does not require root privileges.
- *Readiness.* **Storage and auth, not provider health.** A temporary model outage must not restart a healthy state server.
- *Browser security.* **No direct bearer token embedding.** Trusted application backends own App Server credentials.

**Open questions**

- *Channel host ownership.* Is the existing Letta Code channel host reused unchanged or extracted into a smaller pinned package?
- *Deployment repository.* Should `letta-app-server-deployment` gain a Lotta profile or should Lotta use a separate deployment repository?
