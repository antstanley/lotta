# 04 — Persistence and MemFS

**Status:** Draft · **Date:** 2026-08-14 · **Owner:** Ant Stanley · **Scope:** Repo-wide

This page defines local state durability and compatibility with Letta Code's local backend directory. The default root is `~/.letta/lc-local-backend`; `LETTA_LOCAL_BACKEND_DIR` overrides it. Additional schedules, channel state, and settings remain under the baseline `~/.letta` and project paths.

---

## Responsibilities

1. Persist agents, conversations, transcripts, compiled prompts, and provider authentication locally.
2. Read and write the schema-v2 transcript format used by the pinned reference.
3. Initialize and manage one Git-backed MemFS repository per agent.
4. Preserve the baseline's accepted storage shapes and repair behavior.
5. Add atomic-write, conflict-detection, bounded-migration, backup, and restore hardening without making TypeScript state unreadable.
6. Keep persistence adapters below the domain/runtime layers.

---

## Local-backend directory layout

```text
${LETTA_LOCAL_BACKEND_DIR:-~/.letta/lc-local-backend}/
├── agents/
│   └── <base64url(agent-id)>.json
├── conversations/
│   ├── <base64url("default:" + agent-id)>/
│   └── <base64url("conversation:" + conversation-id)>/
│       ├── conversation.json
│       ├── manifest.json
│       ├── messages.jsonl
│       └── system-prompt.json
├── memfs/
│   └── <agent-id>/memory/
│       ├── .git/
│       ├── system/
│       ├── skills/
│       └── mods/
└── providers/
    └── auth.json
```

Named-conversation directory keys are not agent-scoped on disk. The Rust implementation preserves both key forms exactly for cross-runtime tests.

`providers/auth.json` is the baseline version-1 provider record map. It contains provider configuration and raw API/OAuth credentials, with the directory created as mode `0700` and the file forced to mode `0600`. Live at-rest encryption is not part of the pinned storage contract: the TypeScript runtime cannot read a `credentials.enc` replacement.

Derived indexes live under `indexes/` and are disposable. JSON, JSONL, `auth.json`, and Git remain canonical.

---

## State outside the backend root

Cross-runtime backup and round-trip coverage includes these baseline paths:

```text
~/.letta/settings.json
${LETTA_HOME:-~/.letta}/crons.json
${LETTA_HOME:-~/.letta}/runs/<schedule-id>.jsonl
~/.letta/channels/
├── pending-control-requests.json
└── <channel-id>/
    ├── config.yaml
    ├── accounts.json
    ├── routing.yaml
    ├── pairing.yaml
    └── targets.json

<workspace>/.letta/
├── settings.json
└── settings.local.json
```

Channel plugins can add plugin-owned files below their channel directory. Backup treats those files as opaque but includes them. Project-local settings are included only when their workspace is in backup scope.

---

## Agent and conversation records

Agent files contain normalized `LocalAgentRecord` JSON. Unknown compatible fields are preserved during read-modify-write so a newer reference runtime does not lose data when Lotta touches a known field.

`conversation.json` is a formatted snapshot. `system-prompt.json` uses the exact compiled-prompt shape defined below. The baseline polls record mtimes and refreshes loaded agent/conversation snapshots when another process writes them.

The baseline writes records in place and does not take a local-backend process lock. Lotta adds these durability mechanics:

1. serialize to a same-directory temporary file,
2. flush file contents,
3. rename atomically,
4. flush the parent directory where supported,
5. compare the source mtime/revision immediately before replacement and return `storage_conflict` if a lock-ignorant process changed it.

A Lotta advisory lock serializes Lotta writers only. It is not presented as protection from a concurrently running TypeScript process, because that process does not participate. Lotta continues to refresh mtimes and rejects observed external conflicts, but a TypeScript write can race between comparison and rename. Concurrent mixed-runtime writes to one root are unsupported; cross-runtime compatibility means sequential round trips or a quiesced handoff.

---

## Transcript contract

`manifest.json` contains:

```json
{
  "schema_version": 2,
  "message_format": "pi-session-entry-jsonl",
  "provider_stack": "pi-ai",
  "created_at": "<RFC3339>"
}
```

Optional migration fields are `migrated_from`, `migrated_at`, and `backup_path`. The pinned migration writes `migrated_from` and `backup_path`; Lotta also writes `migrated_at`.

A newly written `messages.jsonl` starts with one session header:

```json
{"type":"session","version":3,"id":"...","timestamp":"...","cwd":"..."}
```

Subsequent rows are `message` or `compaction` entries defined in [01-domain-model.md](01-domain-model.md). Ordinary turns and compactions append one complete entry line. Compaction does not rewrite or back up the active transcript.

Full transcript rewrites occur for explicit migration/legacy-format upgrade, conversation fork, and initial/full persistence of an already loaded conversation. Migration writes a timestamped source backup first; ordinary full rewrites do not.

### Baseline-compatible loading

Lotta accepts the same schema-v2 rows the reference accepts:

- valid JSONL and a supported manifest version/format/provider stack,
- message and compaction rows even when a session header is absent,
- duplicate entry IDs or unverified parent links as the baseline currently does,
- compaction references without enforcing a retained-entry graph on load.

Non-message/compaction rows are skipped during active-message projection. Orphan tool results are removed from the active projection; affected `in_context_message_ids` are repaired and `conversation.json` is persisted while the source transcript is left unchanged. Oversized tool results follow the baseline clipping/repair path.

An unversioned non-empty transcript maps the baseline `LocalTranscriptMigrationRequiredError` to Lotta code `transcript_migration_required`. A versioned transcript containing legacy UI-message rows maps `LocalTranscriptRepairRequiredError` to `transcript_repair_required`. A versioned legacy `pi-ai-message-jsonl` transcript is upgraded on its next non-empty persistence, matching the baseline.

### Strict verification

`lotta local-backend verify` reports duplicate entry IDs, invalid/backward parent links, absent/multiple headers, orphan tool results, invalid compaction references, and unsupported fields without mutating state. These Tiger Style checks are diagnostics, not stricter startup rejection that would break import compatibility.

---

## Migration

The CLI exposes:

```text
lotta local-backend migrate-transcripts \
  --storage-dir <path> [--dry-run]
```

Migration preserves the baseline conversion and `in_context_message_ids` remapping. Lotta adds bounded scanning, output validation, and atomic replacement after the timestamped backup is durable. A failed conversion leaves the original active. Re-running a completed migration is idempotent. These mechanics are Lotta hardening; the pinned TypeScript migration writes converted files in place after copying the backup.

Cross-runtime fixtures cover:

1. current TypeScript state read by Rust,
2. Rust-written state read by TypeScript,
3. unversioned and versioned-legacy migration,
4. versioned rows tolerated by the baseline loader,
5. orphan-result active-projection repair,
6. interrupted append and interrupted replacement recovery,
7. corrupt/unsupported manifests rejected without mutation.

---

## MemFS

Each agent memory directory is an independent Git repository. Agent creation renders memory blocks as Markdown files:

```text
system/<normalized-label>.md
```

Labels already beginning with `system/` remain there; other labels are placed below `system/`. Backslashes and a trailing `.md` are normalized, empty/absolute-looking labels collapse through the same segment normalization as the baseline, and `.` or `..` segments are rejected. A missing description becomes `Memory block <label>` so every generated file still receives non-empty YAML frontmatter.

MemFS operations support:

- initialize with deterministic initial files,
- status, tree, read, write, delete, and rename,
- history and file-at-revision,
- diff and commit,
- worktree-based reflection/merge,
- pre-commit validation of memory Markdown,
- optional post-commit push to `letta.memoryRepository.url`.

Prompt freshness is not a post-commit hook. It is detected by comparing the committed MemFS revision at compilation time.

Local backend MemFS has no implicit Letta remote. A user-configured Git remote can be pulled or pushed by explicit memory-repository operations and the optional post-commit hook. The server does not contact Letta Cloud to synchronize memory.

---

## Prompt compilation

Compilation inputs include the managed/custom system text, committed memory files and tree, selected skills, runtime reminders, and tool/model guidance. The persisted compatibility record has exactly:

- `content`,
- `coreMemory`,
- optional `midConversationSystemPrompt`,
- `compiledAt`,
- `rawSystemHash`,
- optional `memfsRevision`.

Cache reuse compares `rawSystemHash` and `memfsRevision`. Model/toolset inputs and a rendered-content hash are not persisted in `system-prompt.json`. An uncommitted memory working tree is visible to tools but does not become authoritative prompt memory until committed.

---

## Durability and bounds

These values are Lotta hardening unless a baseline value is named explicitly.

| Constant | Default | Enforcement |
|---|---:|---|
| `AGENTS_MAX` | 100,000 | Reject create |
| `CONVERSATIONS_PER_AGENT_MAX` | 100,000 | Reject create |
| `TRANSCRIPT_LINE_BYTES_MAX` | 8 MiB | Reject append/read |
| `TRANSCRIPT_BYTES_MAX` | 16 GiB | Require archive/export before append |
| `MEMORY_FILE_BYTES_MAX` | 8 MiB | Reject write |
| `MEMORY_FILES_MAX` | 100,000 | Reject create |
| `MIGRATION_BACKUPS_PER_FILE_MAX` | 3 | Rotate oldest completed Lotta backup |
| `LOTTA_STORAGE_LOCK_WAIT_MS` | 0 | Fail a competing Lotta writer; external TypeScript writers are detected by conflict checks |
| `ATOMIC_WRITE_RETRIES_MAX` | 3 | Return I/O or conflict error after bounded retry |
| `CRON_RUN_LOG_KEEP_LINES` | 2,000 | Baseline line-retention bound |
| `CRON_RUN_LOG_BYTES_MAX` | 2,000,000 | Baseline per-task JSONL byte bound |

Disk-full, permission, parse, checksum, conflict, and Lotta-lock errors are distinct. Error logs include paths but never file contents or credentials.

---

## Backup and restore

A consistent backup pauses new Lotta durable admissions, waits for active commits, snapshots the local-backend root and selected external state, and bundles MemFS repositories. It samples source metadata before and after copying and aborts on an observed lock-ignorant write. Operators must quiesce TypeScript writers because metadata checks cannot close every race.

The live `providers/auth.json` remains baseline-compatible plaintext protected by filesystem permissions. Backup archives containing it are encrypted with an operator-supplied key. Restore decrypts into a new directory, validates the complete snapshot, applies secure modes, and swaps roots only after validation.

---

## Assumptions and open questions

**Assumptions**

- Local filesystems provide same-directory atomic rename.
- Git is available as an external executable in the first implementation.
- JSON field preservation is sufficient for forward-compatible records within one support window.

**Decisions**

- *Canonical store.* **Reference-compatible JSON, JSONL, `auth.json`, side stores, and Git.** Existing local agents can move between Letta Code and Lotta without an export step.
- *Conversation keys.* **Preserve the two baseline key forms exactly.** The default key includes the agent; a named key includes only the conversation ID.
- *Durability hardening.* **Atomic replacement plus best-effort external-change detection.** The TypeScript runtime ignores Lotta locks, so simultaneous mixed-runtime writes are unsupported rather than hidden behind a false global lock guarantee.
- *Transcript tolerance.* **Load what the baseline loads; verify more strictly out of band.** Diagnostic invariants do not narrow accepted existing state.
- *Provider credentials.* **Keep the live `auth.json` compatibility surface and encrypt backups.** Replacing it with an encrypted live file would fail TypeScript round trips.
- *Automatic migration.* **Only the baseline's versioned legacy upgrade occurs on ordinary persistence.** Unversioned conversion and repair remain explicit commands with backups.

**Open questions**

- *Live credential encryption.* Should a future change spec introduce keyring/encrypted provider storage with an explicit TypeScript migration path?
- *Remote memory repositories.* Which Git authentication mechanisms are in scope beyond local filesystem and SSH agent?
