import {
  closeSync,
  constants,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { createHash } from "node:crypto";
import { fileURLToPath, pathToFileURL } from "node:url";

const REQUEST_BYTES_MAX = 1_048_576;
const RESPONSE_BYTES_MAX = 1_048_576;
const RUNNER_DIR = dirname(fileURLToPath(import.meta.url));
const PINNED_ROOT = resolve(RUNNER_DIR, "../../../../../letta-code");
const MANIFEST_PATH = join(RUNNER_DIR, "pinned-sources.json");
const SOURCE_MANIFEST = JSON.parse(readFileSync(MANIFEST_PATH, "utf8"));
const SOURCE_PATHS = Object.keys(SOURCE_MANIFEST.sources);
const sourcePath = (relativePath) => join(PINNED_ROOT, relativePath);
const LOCAL_STORE_PATH = sourcePath("src/backend/local/local-store.ts");
const TRANSCRIPT_MIGRATION_PATH = sourcePath("src/backend/local/transcript-migration.ts");
const PROVIDER_PATH = sourcePath("src/backend/local/local-provider-auth-store.ts");
const CRON_PATH = sourcePath("src/cron/cron-file.ts");
const RUN_LOG_PATH = sourcePath("src/cron/run-log.ts");
const SETTINGS_PATH = sourcePath("src/settings-manager.ts");
const CHANNEL_PATHS = Object.fromEntries(
  ["config", "accounts", "routing", "pairing", "targets", "pending-control-requests"]
    .map((name) => [name, sourcePath(`src/channels/${name}.ts`)]),
);
const VERIFIED_SOURCES = await verifySources();
if (!existsSync(join(PINNED_ROOT, "node_modules"))) {
  throw new Error(
    `pinned dependencies missing; run bun install --frozen-lockfile in ${PINNED_ROOT}`,
  );
}

let input = "";
for await (const chunk of process.stdin) {
  input += chunk;
  if (Buffer.byteLength(input) > REQUEST_BYTES_MAX) throw new Error("request_bytes_max exceeded");
}
const request = JSON.parse(input);
const roots = [request.root, request.backend, request.home, request.lettaHome, request.workspace]
  .map((value) => resolve(String(value)));
let pathFailure;
try {
  for (const root of roots) assertConfined(root, roots[0]);
} catch (error) {
  pathFailure = error;
}
process.env.HOME = request.home;
process.env.LETTA_HOME = request.lettaHome;
process.env.LETTA_LOCAL_BACKEND_DIR = request.backend;
let modules;
let lease;
try {
  if (pathFailure) throw pathFailure;
  lease = acquireLease(request.root);
  modules = await loadModules();
  globalThis.harnessModules = modules;
  modules.channelConfig.__testOverrideChannelsRoot(
    join(request.home, ".letta", "channels"),
  );
  const result = await execute(request, modules);
  respond({ ok: true, operation: request.operation, artifact: request.artifact, ...result });
} catch (error) {
  const failure = {
    ok: false,
    path: String(error?.path ?? request.root),
    field: String(error?.field ?? "operation"),
    operation: request.operation,
    artifact: request.artifact,
    message: sanitize(String(error?.message ?? error)),
  };
  respond(failure);
  process.exitCode = 1;
} finally {
  if (lease) rmSync(lease, { force: true });
}

async function execute(request, modules) {
  const key = `${request.operation}:${request.artifact}`;
  if (request.operation === "provenance") return provenance();
  if (request.operation === "hold_lease") return holdLease(request);
  if (request.operation === "overflow_stdout") return overflow(process.stdout);
  if (request.operation === "overflow_stderr") return overflow(process.stderr);
  if (request.operation === "backend_write") return backendWrite(request);
  if (request.operation === "backend_read_update") return backendReadUpdate(request);
  if (request.operation === "side_write") return sideWrite(request, modules);
  if (request.operation === "side_read_update") return sideReadUpdate(request, modules);
  if (request.operation === "migration_load") return migrationLoad(request);
  if (request.operation === "migration_validate") return migrationValidate(request);
  if (request.operation === "migration_persist") return migrationPersist(request);
  if (request.operation === "migration_write_current") return migrationWriteCurrent(request);
  if (request.operation === "migration_read_current") return migrationReadCurrent(request);
  if (request.operation === "migration_convert") return migrationConvert(request);
  throw new Error(`unsupported operation ${key}`);
}

async function holdLease(request) {
  const local = store(request);
  local.ensureAgent("agent-local-fixture");
  local.updateAgent("agent-local-fixture", { name: "TS holder agent" });
  writeFileSync(join(request.root, "holder-ready"), "ready\n");
  const release = join(request.root, "holder-release");
  const deadline = Date.now() + 30_000;
  while (!existsSync(release)) {
    if (Date.now() >= deadline) throw new Error("holder release timeout");
    await Bun.sleep(5);
  }
  return { value: { phase: "holder-released" }, provenance: provenance() };
}

async function overflow(stream) {
  stream.write(Buffer.alloc(RESPONSE_BYTES_MAX + 1, 120));
  await Bun.sleep(30_000);
  return { value: null };
}

function store(request) {
  return new globalThis.harnessModules.LocalStore("agent-local-fixture", {
    storageDir: request.backend,
    seedDefaultAgent: false,
    strictAgentAccess: false,
    strictConversationAccess: false,
    defaultAgentModel: "openai/gpt-4.1-mini",
  });
}

function backendWrite(request) {
  const local = store(request);
  if (request.artifact === "agent") {
    local.ensureAgent("agent-local-fixture");
    injectUnknown(agentPath(request), "ts_extension", { nested: [1, { retained: true }] });
    local.updateAgent("agent-local-fixture", { name: "TS Agent" });
  } else if (request.artifact === "conversation") {
    local.ensureAgent("agent-local-fixture");
    injectUnknown(conversationPath(request), "ts_extension", { retained: "conversation" });
    const snapshot = readJson(conversationPath(request));
    if (snapshot.id !== "default") throw new Error("unexpected default conversation");
  } else if (request.artifact === "transcript") {
    local.appendTurnInput("default", {
      agent_id: "agent-local-fixture",
      messages: [{ role: "user", content: "synthetic hello" }],
    });
    local.compactConversationAll({
      conversationId: "default",
      agentId: "agent-local-fixture",
      summary: "synthetic summary",
      packedSummary: "synthetic packed summary",
    });
  } else if (request.artifact === "manifest") {
    local.ensureAgent("agent-local-fixture");
  } else if (request.artifact === "system_prompt") {
    local.setCompiledSystemPrompt("default", "agent-local-fixture", promptRecord());
    if (!local.getCompiledSystemPrompt("default", "agent-local-fixture")) {
      throw new Error("compiled prompt missing after set");
    }
  } else throw new Error("unsupported backend artifact");
  return { value: backendValue(request), provenance: provenance() };
}

function backendReadUpdate(request) {
  const local = store(request);
  if (request.artifact === "agent") {
    local.retrieveAgentRecord("agent-local-fixture");
  } else if (request.artifact === "conversation") {
    local.listConversationMessages("default", { agent_id: "agent-local-fixture" });
  } else if (request.artifact === "transcript") {
    local.listLocalMessages("default", "agent-local-fixture");
  } else if (request.artifact === "manifest") {
    local.listLocalMessages("default", "agent-local-fixture");
  } else if (request.artifact === "system_prompt") {
    const value = local.getCompiledSystemPrompt("default", "agent-local-fixture");
    if (!value) throw new Error("Rust compiled prompt not loaded");
    local.setCompiledSystemPrompt("default", "agent-local-fixture", value);
  } else throw new Error("unsupported backend artifact");
  return { value: backendValue(request), provenance: provenance() };
}

function backendValue(request) {
  if (request.artifact === "agent") return readJson(agentPath(request));
  if (request.artifact === "conversation") return readJson(conversationPath(request));
  if (request.artifact === "transcript") return readJsonl(messagesPath(request));
  if (request.artifact === "manifest") return readJson(manifestPath(request));
  if (request.artifact === "system_prompt") return readJson(promptPath(request));
  throw new Error("unknown backend value");
}

async function sideWrite(request, modules) {
  ensureSideRoots(request);
  if (request.artifact === "provider_auth") {
    globalThis.harnessModules.provider.createOrUpdateLocalProvider({
      providerType: "openai",
      providerName: "synthetic-openai",
      apiKey: "<redacted-fixture>",
      storageDir: request.backend,
    });
  } else if (request.artifact === "crons") {
    globalThis.harnessModules.cron.addTask({
      agent_id: "agent-local-fixture",
      conversation_id: "default",
      name: "synthetic schedule",
      description: "synthetic",
      cron: "0 * * * *",
      timezone: "UTC",
      recurring: true,
      prompt: "synthetic prompt",
    });
  } else if (request.artifact === "run_log") {
    const path = globalThis.harnessModules.runLog.getCronRunLogPath(
      "schedule-fixture",
    );
    globalThis.harnessModules.runLog.appendCronRunLog(path, {
      ts: 1_700_000_000_000,
      jobId: "schedule-fixture",
      action: "finished",
      status: "ok",
      summary: "synthetic",
    });
  } else if (request.artifact === "settings") {
    const manager = modules.settings.settingsManager;
    await manager.initialize();
    manager.updateSettings({
      model: "openai/gpt-4.1-mini",
      harnessExtension: { retained: true },
    });
    await manager.flush();
    if (manager.getSetting("model") !== "openai/gpt-4.1-mini") {
      throw new Error("settings update was not retained");
    }
  } else if (request.artifact === "channels") {
    writeChannelConfigAndPlugin(request);
    writeChannelsWithModules(modules);
  } else throw new Error("unsupported side artifact");
  return { value: sideValue(request), provenance: provenance() };
}

async function sideReadUpdate(request, modules) {
  if (request.artifact === "provider_auth") {
    const records = globalThis.harnessModules.provider.listLocalProviderRecords(
      request.backend,
    );
    if (records.length === 0) throw new Error("provider store empty");
    const record = records[0];
    if (record.name !== "synthetic-openai") {
      throw new Error("unexpected provider record");
    }
  } else if (request.artifact === "crons") {
    globalThis.harnessModules.cron.readCronFile();
  } else if (request.artifact === "run_log") {
    const path = globalThis.harnessModules.runLog.getCronRunLogPath(
      "schedule-fixture",
    );
    const entries = globalThis.harnessModules.runLog.readCronRunLogEntries(path);
    if (entries.length === 0) throw new Error("run log empty");
  } else if (request.artifact === "settings") {
    const manager = modules.settings.settingsManager;
    await manager.initialize();
    const loaded = manager.getSettings();
    if (manager.getSetting("model") !== loaded.model) {
      throw new Error("settings getSetting disagrees with getSettings");
    }
  } else if (request.artifact === "channels") {
    readChannelsWithModules(modules);
  } else throw new Error("unsupported side artifact");
  return { value: sideValue(request), provenance: provenance() };
}

function projectMessages(messages) {
  return messages.map((message) => ({
    id: message.id,
    role: message.role,
    text: Array.isArray(message.content)
      ? message.content.filter((part) => part.type === "text").map((part) => part.text).join("")
      : Array.isArray(message.parts)
        ? message.parts.filter((part) => part.type === "text").map((part) => part.text).join("")
        : typeof message.content === "string" ? message.content : "",
  }));
}

function migrationSnapshot(request, local) {
  const conversation = readJson(conversationPath(request));
  return {
    agent: local.retrieveAgentRecord("agent-local-fixture"),
    conversation,
    messages: projectMessages(local.listLocalMessages(conversation.id, "agent-local-fixture")),
    manifest: readJson(manifestPath(request)),
  };
}

function migrationWriteCurrent(request) {
  const local = store(request);
  local.ensureAgent("agent-local-fixture");
  local.updateAgent("agent-local-fixture", { name: "TS Migration Agent" });
  local.appendTurnInput("default", {
    agent_id: "agent-local-fixture",
    messages: [{ role: "user", content: "TS current migration message" }],
  });
  local.compactConversationAll({
    conversationId: "default",
    agentId: "agent-local-fixture",
    summary: "TS current migration summary",
    packedSummary: "TS current packed summary",
  });
  local.appendTurnInput("default", {
    agent_id: "agent-local-fixture",
    messages: [{ role: "user", content: "TS current representative message" }],
  });
  return { value: migrationSnapshot(request, local), provenance: provenance() };
}

function migrationReadCurrent(request) {
  const local = store(request);
  return { value: migrationSnapshot(request, local), provenance: provenance() };
}

function migrationConvert(request) {
  const value = globalThis.harnessModules.transcriptMigration
    .migrateLocalBackendTranscripts({ storageDir: request.backend, dryRun: false });
  return { value, provenance: provenance() };
}

function migrationLoad(request) {
  if (request.artifact === "orphan") {
    return migrationProject(request);
  }
  const local = store(request);
  const conversationId = readJson(conversationPath(request)).id;
  let value;
  if (
    request.artifact === "current" ||
    request.artifact === "tolerated" ||
    request.artifact === "orphan"
  ) {
    value = local.listLocalMessages(conversationId, "agent-local-fixture");
  } else if (request.artifact === "legacy") {
    try {
      value = local.listLocalMessages(conversationId, "agent-local-fixture");
    } catch (error) {
      value = { rejected: true, name: error.constructor.name };
    }
  } else if (request.artifact === "interrupted") {
    try {
      value = local.listLocalMessages(conversationId, "agent-local-fixture");
    } catch (error) {
      value = { rejected: true, name: error.constructor.name };
    }
  } else if (request.artifact === "invalid") {
    try {
      local.listLocalMessages("default", "agent-local-fixture");
      throw new Error("invalid manifest accepted");
    } catch (error) {
      if (error.message === "invalid manifest accepted") throw error;
      value = { rejected: true, name: error.constructor.name };
    }
  } else throw new Error("unsupported migration artifact");
  return { value, provenance: provenance() };
}

function migrationProject(request) {
  const named = join(request.backend, "conversations",
    Buffer.from("conversation:conversation-fixture").toString("base64url"), "conversation.json");
  const canonical = conversationPath(request);
  if (!existsSync(canonical) && !existsSync(named)) {
    throw new Error(`missing migration source: canonical=${canonical}`);
  }
  const sourcePath = existsSync(canonical) ? canonical : named;
  const source = readJson(sourcePath);
  const sourceDirectory = dirname(sourcePath);
  const key = Buffer.from(`conversation:${source.id}`).toString("base64url");
  const directory = join(request.backend, "conversations", key);
  const local = store(request);
  const messages = local.listLocalMessages(source.id, source.agent_id);
  const conversation = join(directory, "conversation.json");
  return {
    value: {
      messages,
      source: {
        key: sourceDirectory.split("/").at(-1),
        directory: relative(request.backend, sourceDirectory),
      },
      actual: {
        key,
        directory: relative(request.backend, directory),
        conversation: relative(request.backend, conversation),
        exists: existsSync(conversation),
        value: existsSync(conversation) ? readJson(conversation) : null,
      },
    },
    provenance: provenance(),
  };
}

function migrationValidate(request) {
  const conversation = conversationPath(request);
  if (!existsSync(conversation)) {
    throw Object.assign(new SyntaxError("missing strict conversation fixture"), {
      path: conversation,
    });
  }
  const local = new globalThis.harnessModules.LocalStore("agent-local-fixture", {
    storageDir: request.backend,
    seedDefaultAgent: false,
    strictAgentAccess: true,
    strictConversationAccess: true,
    defaultAgentModel: "openai/gpt-4.1-mini",
  });
  let value;
  try {
    local.listLocalMessages("default", "agent-local-fixture");
    throw new Error("invalid manifest accepted");
  } catch (error) {
    if (error.message === "invalid manifest accepted") throw error;
    value = { rejected: true, name: error.constructor.name };
  }
  return { value, provenance: provenance() };
}

function migrationPersist(request) {
  const local = store(request);
  if (request.artifact === "current") {
    local.appendTurnInput("default", {
      agent_id: "agent-local-fixture",
      messages: [{ role: "user", content: "Rust handoff" }],
    });
  } else if (request.artifact === "legacy") {
    globalThis.harnessModules.transcriptMigration.migrateLocalBackendTranscripts({
      storageDir: request.backend,
      dryRun: false,
    });
    const upgraded = store(request);
    upgraded.appendTurnInput(readJson(conversationPath(request)).id, {
      agent_id: "agent-local-fixture",
      messages: [{
        role: "user",
        content: [{ type: "text", text: "TS versioned upgrade append" }],
      }],
    });
    if (!readFileSync(messagesPath(request), "utf8").includes("TS versioned upgrade append")) {
      upgraded.compactConversationAll({
        conversationId: readJson(conversationPath(request)).id,
        agentId: "agent-local-fixture",
        summary: "TS versioned upgrade summary",
        packedSummary: "TS versioned upgrade append",
      });
    }
  } else {
    return migrationLoad(request);
  }
  return { value: readJsonl(messagesPath(request)), provenance: provenance() };
}

function sideValue(request) {
  if (request.artifact === "provider_auth") {
    return readJson(join(request.backend, "providers/auth.json"));
  }
  if (request.artifact === "crons") return readJson(join(request.lettaHome, "crons.json"));
  if (request.artifact === "run_log") {
    return readJsonl(join(request.lettaHome, "runs/schedule-fixture.jsonl"));
  }
  if (request.artifact === "settings") return readJson(settingsPath(request));
  if (request.artifact === "channels") return channelInventory(request);
  throw new Error("unknown side value");
}

function writeChannelConfigAndPlugin(request) {
  const channel = join(request.home, ".letta", "channels", "telegram");
  mkdirSync(join(channel, "plugin/nested"), { recursive: true });
  const config = "enabled: true\nagent_id: agent-local-fixture\n";
  writeFileSync(join(channel, "config.yaml"), config);
  writeFileSync(join(channel, "plugin/nested/opaque.bin"), "SANITIZED_PLUGIN_BYTES");
}

function writeChannelsWithModules(modules) {
  modules.accounts.upsertChannelAccount("telegram", telegramAccount());
  modules.accounts.loadChannelAccounts("telegram");
  assertCount(modules.accounts.listChannelAccounts("telegram"), "accounts");
  modules.routing.addRoute("telegram", telegramRoute());
  modules.routing.clearAllRoutes();
  modules.routing.loadRoutes("telegram");
  if (!modules.routing.getRoute("telegram", "chat-1", "account-1")) {
    throw new Error("route missing");
  }
  modules.pairing.createPairingCode("telegram", "sender-1", "chat-1", "synthetic");
  modules.pairing.loadPairingStore("telegram");
  assertCount(modules.pairing.getPendingPairings("telegram"), "pairings");
  modules.targets.upsertChannelTarget("telegram", telegramTarget());
  modules.targets.loadTargetStore("telegram");
  assertCount(modules.targets.listChannelTargets("telegram"), "targets");
  modules.pending.upsertPendingControlRequest(controlRequest());
  assertCount(modules.pending.listPendingControlRequests(), "pending controls");
  if (!modules.channelConfig.readChannelConfig("telegram")) throw new Error("config missing");
}

function readChannelsWithModules(modules) {
  modules.accounts.clearChannelAccountStores();
  modules.accounts.loadChannelAccounts("telegram");
  assertCount(modules.accounts.listChannelAccounts("telegram"), "accounts");
  modules.routing.clearAllRoutes();
  modules.routing.loadRoutes("telegram");
  if (!modules.routing.getRoute("telegram", "chat-1", "account-1")) {
    throw new Error("route missing");
  }
  modules.pairing.clearPairingStores();
  modules.pairing.loadPairingStore("telegram");
  assertCount(modules.pairing.getPendingPairings("telegram"), "pairings");
  modules.targets.clearTargetStores();
  modules.targets.loadTargetStore("telegram");
  assertCount(modules.targets.listChannelTargets("telegram"), "targets");
  modules.pending.clearPendingControlRequestStore();
  assertCount(modules.pending.listPendingControlRequests(), "pending controls");
  if (!modules.channelConfig.readChannelConfig("telegram")) throw new Error("config missing");
}

function assertCount(values, artifact) {
  if (values.length === 0) throw new Error(`${artifact} empty`);
}

function telegramAccount() {
  return {
    channel: "telegram", accountId: "account-1", displayName: "Telegram", enabled: true,
    dmPolicy: "pairing", allowedUsers: [], binding: { agentId: null, conversationId: null },
    groupMode: "open", transcribeVoice: false, richPrivateChatDefault: true,
    richDraftStreaming: false, createdAt: "2026-08-14T00:00:00.000Z",
    updatedAt: "2026-08-14T00:00:00.000Z",
  };
}

function telegramRoute() {
  return {
    accountId: "account-1", chatId: "chat-1", chatType: "direct", threadId: null,
    agentId: "agent-local-fixture", conversationId: "default", enabled: true,
    createdAt: "2026-08-14T00:00:00.000Z", updatedAt: "2026-08-14T00:00:00.000Z",
  };
}

function telegramTarget() {
  return {
    accountId: "account-1", targetId: "chat-1", targetType: "direct", chatId: "chat-1",
    label: "synthetic", discoveredAt: "2026-08-14T00:00:00.000Z",
    lastSeenAt: "2026-08-14T00:00:00.000Z", lastMessageId: "message-1",
  };
}

function controlRequest() {
  return {
    requestId: "request-1",
    source: {
      channel: "telegram", chatId: "chat-1", agentId: "agent-local-fixture",
      conversationId: "default",
    },
    toolName: "synthetic_tool", input: { synthetic: true },
  };
}

function channelInventory(request) {
  const root = join(request.home, ".letta", "channels");
  const paths = [
    "pending-control-requests.json",
    "telegram/config.yaml",
    "telegram/accounts.json",
    "telegram/routing.yaml",
    "telegram/pairing.yaml",
    "telegram/targets.json",
    "telegram/plugin/nested/opaque.bin",
  ];
  return Object.fromEntries(paths.map((path) => [path, readFileSync(join(root, path), "utf8")]));
}

function ensureSideRoots(request) {
  for (const path of [request.backend, request.home, request.lettaHome, request.workspace]) {
    mkdirSync(path, { recursive: true });
  }
  mkdirSync(dirname(settingsPath(request)), { recursive: true });
}

function promptRecord() {
  return {
    content: "synthetic system prompt",
    coreMemory: "synthetic memory",
    compiledAt: "2026-08-14T00:00:00.000Z",
    rawSystemHash: "a".repeat(64),
    memfsRevision: "synthetic-revision",
  };
}

function injectUnknown(path, field, value) {
  const record = readJson(path);
  record[field] = value;
  writeFileSync(path, `${JSON.stringify(record, null, 2)}\n`);
}

function agentPath(request) {
  const name = Buffer.from("agent-local-fixture").toString("base64url");
  return join(request.backend, "agents", `${name}.json`);
}
function conversationDir(request) {
  const name = Buffer.from("default:agent-local-fixture").toString("base64url");
  return join(request.backend, "conversations", name);
}
function conversationPath(request) {
  return join(conversationDir(request), "conversation.json");
}
function messagesPath(request) { return join(conversationDir(request), "messages.jsonl"); }
function manifestPath(request) { return join(conversationDir(request), "manifest.json"); }
function promptPath(request) { return join(conversationDir(request), "system-prompt.json"); }
function settingsPath(request) { return join(request.home, ".letta", "settings.json"); }
function readJson(path) { return JSON.parse(readFileSync(path, "utf8")); }
function readJsonl(path) {
  return readFileSync(path, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
}

function acquireLease(root) {
  const path = join(root, "runtime-owner.json");
  mkdirSync(root, { recursive: true });
  const flags = constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY;
  const descriptor = openSync(path, flags, 0o600);
  const owner = JSON.stringify({ owner: "typescript", pid: process.pid });
  writeFileSync(descriptor, owner);
  closeSync(descriptor);
  return path;
}

function assertConfined(path, root) {
  const rel = relative(root, path);
  if (rel.startsWith("..") || rel === ".." || resolve(path) === resolve("/")) {
    throw new Error(`path escapes isolated root: ${sanitize(path)}`);
  }
}

async function loadModules() {
  const loaded = await Promise.all([
    import(pathToFileURL(LOCAL_STORE_PATH).href),
    import(pathToFileURL(TRANSCRIPT_MIGRATION_PATH).href),
    import(pathToFileURL(PROVIDER_PATH).href),
    import(pathToFileURL(CRON_PATH).href),
    import(pathToFileURL(RUN_LOG_PATH).href),
    import(pathToFileURL(SETTINGS_PATH).href),
    ...Object.values(CHANNEL_PATHS).map((path) => import(pathToFileURL(path).href)),
  ]);
  const [local, transcriptMigration, provider, cron, runLog, settings, ...channels] = loaded;
  return {
    LocalStore: local.LocalStore, transcriptMigration, provider, cron, runLog, settings,
    channelConfig: channels[0], accounts: channels[1], routing: channels[2],
    pairing: channels[3], targets: channels[4], pending: channels[5],
  };
}

async function verifySources() {
  const verified = {};
  for (const relativePath of SOURCE_PATHS) {
    const path = sourcePath(relativePath);
    if (!existsSync(path)) throw new Error(`missing pinned source ${relativePath}`);
    const sha256 = await hashFile(path);
    const expected = SOURCE_MANIFEST.sources[relativePath];
    if (sha256 !== expected) {
      throw new Error(`pinned source SHA mismatch: ${relativePath}`);
    }
    verified[relativePath] = { url: pathToFileURL(path).href, sha256 };
  }
  return verified;
}

async function hashFile(path) {
  const file = Bun.file(path);
  if (file.size > RESPONSE_BYTES_MAX * 16) throw new Error(`pinned source too large: ${path}`);
  const reader = file.stream().getReader();
  const hash = createHash("sha256");
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) return hash.digest("hex");
    total += value.byteLength;
    if (total > RESPONSE_BYTES_MAX * 16) throw new Error(`pinned source too large: ${path}`);
    hash.update(value);
  }
}

function provenance() {
  return {
    pinnedRoot: PINNED_ROOT,
    sourceCommit: SOURCE_MANIFEST.source_commit,
    sources: VERIFIED_SOURCES,
  };
}

function sanitize(value) {
  return value.replace(/[\u0000-\u001f\u007f]/g, "?").slice(0, 4096);
}

function respond(value) {
  const output = JSON.stringify(value);
  if (Buffer.byteLength(output) > RESPONSE_BYTES_MAX) {
    throw new Error("response_bytes_max exceeded");
  }
  process.stdout.write(output);
}
