import { createServer } from "node:http";import {
  lstat, mkdir, readFile, readdir, writeFile,
} from "node:fs/promises";
import { join } from "node:path";
import { mock } from "bun:test";
import {
  Normalizer, type TimestampAttempt, assertFreshRawResponseIdentities, assertSameStore,
  assertTimestampMutationCoverage,
} from "./openai-capture-adapter";
import {
  assertHandlerDeleteContract, deletePersistedConversation, recordConversationAllocation,
} from "./openai-capture-adapter";
process.on("unhandledRejection", (error) => {
  console.error(error);
  process.exit(1);
});
process.on("uncaughtException", (error) => {
  console.error(error);
  process.exit(1);
});
const TOKEN = "fixture-transport-credential";
const STORAGE = process.env.LETTA_LOCAL_BACKEND_DIR!;
const AGENTS = [
  { id: "agent-local-visible", name: "fixture-visible",
    created_at: "2026-01-01T00:00:00Z", hidden: false },
  { id: "agent-local-collision-a", name: "fixture-collision",
    created_at: "2026-01-02T00:00:00Z", hidden: false },
  { id: "agent-local-collision-b", name: "fixture-collision",
    created_at: "2026-01-03T00:00:00Z", hidden: false },
  { id: "agent-local-hidden", name: "fixture-hidden",
    created_at: "2026-01-04T00:00:00Z", hidden: true },
];
const providerCalls: unknown[] = [];
const rawForkSources: Record<string, string> = {};
type BackendOperation = {
  kind: "create" | "fork"; returned: Record<string, unknown>;
  agent?: unknown; hidden?: unknown; source?: string;
};
const backendOperations = new Map<string, BackendOperation>();
const backendDeleteCalls = new Set<string>();
let providerHeld = false;
let providerRelease: (() => void) | null = null;
let providerGate: Promise<void> | null = null;
let providerArrived: (() => void) | null = null;
function page<T>(items: T[]) {
  return { getPaginatedItems: () => items };
}
function writeProviderEvent(response: any, value: unknown) {
  response.write(`data: ${JSON.stringify(value)}\n\n`);
}
const provider = createServer(async (request, response) => {
  if (request.method === "GET") {
    response.writeHead(200, { "content-type": "application/json" });
    response.end(
      JSON.stringify({
        object: "list",
        data: [{ id: "default", object: "model" }],
      }),
    );
    return;
  }
  const chunks: Buffer[] = [];
  for await (const chunk of request) chunks.push(Buffer.from(chunk));
  providerCalls.push(JSON.parse(Buffer.concat(chunks).toString("utf8")));
  providerArrived?.();
  response.writeHead(200, { "content-type": "text/event-stream" });
  response.flushHeaders();
  if (providerHeld && providerGate) await providerGate;
  writeProviderEvent(response, {
    id: "chatcmpl-11111111-1111-4111-8111-111111111111",
    object: "chat.completion.chunk",
    created: 1767225600,
    model: "default",
    choices: [
      {
        index: 0,
        delta: { role: "assistant", content: "<ASSISTANT_TEXT>" },
        finish_reason: null,
      },
    ],
  });
  writeProviderEvent(response, {
    id: "chatcmpl-11111111-1111-4111-8111-111111111111",
    object: "chat.completion.chunk",
    created: 1767225600,
    model: "default",
    choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
    usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 },
  });
  response.end("data: [DONE]\n\n");
});
await new Promise<void>((resolve) => provider.listen(0, "127.0.0.1", resolve));
const providerPort = (provider.address() as { port: number }).port;
await mkdir(join(STORAGE, "providers"), { recursive: true });
await writeFile(
  join(STORAGE, "providers/auth.json"),
  JSON.stringify(
    {
      version: 1,
      providers: {
        "openai-compatible": {
          id: "local-provider-openai-compatible",
          name: "openai-compatible",
          provider_type: "openai-compatible",
          provider_category: "byok",
          auth: { type: "api", key: "fixture-provider-credential" },
          base_url: `http://127.0.0.1:${providerPort}/v1`,
          created_at: "2026-01-01T00:00:00Z",
          updated_at: "2026-01-01T00:00:00Z",
        },
      },
    },
    null,
    2,
  ),
);
const { HeadlessBackend } = await import("@/backend/dev/fake-headless-backend");
const { ProviderTurnExecutor } = await import("@/backend/dev/provider-turn-executor");
const { PiStreamAdapter } = await import("@/backend/dev/pi-stream-adapter");
const executor = new ProviderTurnExecutor(
  new PiStreamAdapter({ localProviderAuthStorageDir: STORAGE }));
const delegate = new HeadlessBackend(
  "agent-local-visible",
  executor,
  {
    storageDir: STORAGE,
    defaultAgentName: "fixture-visible",
    defaultAgentModel: "openai-compatible/default",
  },
  { modelHandle: "openai-compatible/default" },
);
await delegate.updateAgent("agent-local-visible", {
  name: "fixture-visible",
  system: "",
  model: "openai-compatible/default",
} as never);
function agentProperty(property: PropertyKey) {
  if (property === "listAgents") {
    return async () =>
      page(
        AGENTS.filter((agent) => !agent.hidden).sort((left, right) =>
          left.id.localeCompare(right.id),
        ),
      );
  }
  if (property !== "retrieveAgent") return undefined;
  return async (id: string) => {
    if (id === "agent-local-visible") return delegate.retrieveAgent(id);
    const agent = AGENTS.find((item) => item.id === id);
    if (!agent) throw new Error("not found");
    return agent;
  };
}
const backend = new Proxy(delegate as object, {
  get(target, property, receiver) {
    const agent = agentProperty(property);
    if (agent) return agent;
    if (property === "createConversation") {
      return async (body: Record<string, unknown>) => {
        const value = (await Reflect.get(target, property, receiver)
          .apply(target, [body])) as Record<string, unknown>;
        await recordConversationAllocation(STORAGE, String(value.id));
        backendOperations.set(String(value.id), {
          kind: "create",
          returned: structuredClone(value),
          agent: body.agent_id,
          hidden: body.hidden,
        });
        return value;
      };
    }
    if (property === "deleteConversation") {
      return async (id: string) => {
        const method = Reflect.get(target, property, receiver);
        const value =
          typeof method === "function" ? await method.apply(target, [id]) : {};
        await deletePersistedConversation(STORAGE, id);
        backendOperations.delete(id);
        backendDeleteCalls.add(id);
        return value;
      };
    }
    if (property === "forkConversation") {
      return async (id: string, options: Record<string, unknown> = {}) => {
        const value = (await Reflect.get(target, property, receiver).apply(
          target,
          [id, options],
        )) as Record<string, unknown>;
        const targetId = String(value.id);
        await recordConversationAllocation(STORAGE, targetId);
        if (rawForkSources[targetId] !== undefined) {
          throw new Error(`fork target observed twice: ${targetId}`);
        }
        rawForkSources[targetId] = id;
        backendOperations.set(targetId, {
          kind: "fork",
          returned: structuredClone(value),
          agent: options.agentId,
          hidden: options.hidden,
          source: id,
        });
        return value;
      };
    }
    if (
      property === "createConversationMessageStream" ||
      property === "streamConversationMessages"
    ) {
      return async (...args: unknown[]) => {
        const [conversationId, body, ...rest] = args;
        const bounded = {
          ...(body as object),
          client_tools: [],
          client_skills: [],
        };
        return Reflect.get(target, property, receiver)
          .apply(target, [conversationId, bounded, ...rest]);
      };
    }
    return Reflect.get(target, property, receiver);
  },
});
mock.module("@/backend", () => ({
  getBackend: () => backend,
  __testSetBackend: () => {},
  getLocalBackendStorageDir: () => STORAGE,
  isLocalBackendEnabled: () => true,
  getBackendForMode: () => backend,
  configureBackendMode: () => {},
  configureDevBackend: async () => {},
  isExperimentalLocalBackendEnabled: () => true,
  DEFAULT_CONVERSATION_MESSAGE_ORDER: "desc",
}));
const { settingsManager } = await import("@/settings-manager");
await settingsManager.initialize();
const { handleOpenAiCompatRequest } =
  await import("@/websocket/app-server-openai");
const { parseAppServerWebsocketAuthSettings } =
  await import("@/websocket/app-server-auth");
const authPolicy = parseAppServerWebsocketAuthSettings({
  wsAuth: "capability-token",
  wsTokenSha256:
    "1947de502481746d5dc98a64e8fa1d743d6c3da164f5b031cbf9ee9e0fb05ffb",
});
let baselineRequests = 0;
const baseline = createServer((request, response) => {
  baselineRequests += 1;
  void handleOpenAiCompatRequest(request, response, { authPolicy });
});
await new Promise<void>((resolve) => baseline.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${(baseline.address() as { port: number }).port}`;
function assertIdentityBijection(pairs: Array<{ raw: string; token: string }>) {
  for (const [index, left] of pairs.entries()) for (const right of pairs.slice(index + 1)) {
    if ((left.raw === right.raw) !== (left.token === right.token)) failMutation("raw/token");
  }
}
function rejects(action: () => void) {
  try { action(); } catch { return true; }
  return false;
}
function failMutation(name: string): never {
  throw new Error(`${name} identity collapse or split`);
}
function assertIdentityMutationCoverage() {
  const a = "chatcmpl-11111111-1111-4111-8111-111111111111";
  const b = "chatcmpl-22222222-2222-4222-8222-222222222222";
  const one = "<CHAT_COMPLETION_ID_1>";
  const two = "<CHAT_COMPLETION_ID_2>";
  if (!rejects(() => assertIdentityBijection([{ raw: a, token: one }, { raw: b, token: one }])))
    failMutation("raw collapse");
  if (!rejects(() => assertIdentityBijection([{ raw: a, token: one }, { raw: a, token: two }])))
    failMutation("raw split");
  if (!rejects(() => assertFreshRawResponseIdentities([a, a])))
    failMutation("retry collapse");
  if (!rejects(() => assertFreshRawResponseIdentities([`${a}${b}`])))
    failMutation("response split");
}
function headers(source: Headers) {
  const result: Record<string, string> = {};
  for (const name of ["content-type", "cache-control"]) {
    const value = source.get(name);
    if (value) result[name] = value;
  }
  return result;
}
function parseSse(raw: string, normalizer: Normalizer, attempt: TimestampAttempt) {
  return raw
    .split("\n\n")
    .filter(Boolean)
    .map((block) => {
      let event: string | null = null;
      let data: string | null = null;
      for (const line of block.split("\n")) {
        if (line.startsWith("event: ")) event = line.slice(7);
        else if (line.startsWith("data: ")) data = line.slice(6);
        else throw new Error(`invalid SSE framing: ${line}`);
      }
      if (data === null) throw new Error("SSE block missing data");
      return {
        event,
        data: data === "[DONE]" ? data : normalizer.response(JSON.parse(data), attempt),
      };
    });
}
function materialize(request: any, previousId?: string) {
  const body = structuredClone(request.body);
  if (body?.previous_response_id === "<FROM:responses_stored_json>")
    body.previous_response_id = previousId;
  return body;
}
async function begin(request: any, previousId?: string) {
  const requestHeaders: Record<string, string> = {
    "content-type": "application/json",
    authorization: `Bearer ${TOKEN}`,
  };
  for (const [name, value] of Object.entries(request.headers ?? {}))
    if (name !== "authorization") requestHeaders[name] = String(value);
  return fetch(origin + request.path, {
    method: request.method,
    headers: requestHeaders,
    body: request.body
      ? JSON.stringify(materialize(request, previousId))
      : undefined,
  });
}
async function finish(
  response: Response,
  mode: string,
  normalizer: Normalizer,
) {
  const raw = await response.text();
  const attempt = normalizer.responseAttempt();
  const result: Record<string, unknown> = {
    status: response.status,
    headers: headers(response.headers),
  };
  if (mode === "sse") result.events = parseSse(raw, normalizer, attempt);
  else result.body = normalizer.response(JSON.parse(raw), attempt);
  result.dynamic_map = normalizer.relationships();
  return { result, raw };
}
async function send(request: any, normalizer: Normalizer, previousId?: string) {
  return finish(await begin(request, previousId), request.mode, normalizer);
}
function request(
  method: string,
  path: string,
  mode: string,
  body: unknown = null,
  extra: Record<string, string> = {},
) {
  return {
    method,
    path,
    mode,
    headers: { authorization: "<AUTHORIZATION>", ...extra },
    body,
  };
}
type StoreSnapshot = {
  sequence: number;
  files: Record<string, string>;
  records: Record<string, Record<string, unknown>>;
  transcripts: unknown[];
  artifacts: string[];
  provider: number;
};
async function snapshot(): Promise<StoreSnapshot> {
  const files: Record<string, string> = {};
  await readTree(STORAGE, STORAGE, files);
  const values = Object.entries(files).flatMap(([path, text]) =>
    parseStored(path, text),
  );
  const records: Record<string, Record<string, unknown>> = {};
  for (const value of values) collectConversationRecords(value, records);
  for (const value of values) enrichConversationRecords(value, records);
  validateBackendOperations(records);
  const artifacts = Object.keys(records).sort(sequenceOrder);
  return {
    sequence: durableSequence(values, artifacts),
    files,
    records,
    transcripts: values,
    artifacts,
    provider: providerCalls.length,
  };
}
async function readTree(
  root: string,
  directory: string,
  output: Record<string, string>,
) {
  for (const name of (await readdir(directory)).sort()) {
    const path = join(directory, name);
    const metadata = await lstat(path);
    if (metadata.isSymbolicLink()) throw new Error(`store symlink: ${path}`);
    if (metadata.isDirectory()) await readTree(root, path, output);
    else if (metadata.isFile()) {
      const relative = path.slice(root.length + 1);
      output[relative] = await readFile(path, "utf8");
    }
  }
}
function parseStored(path: string, text: string): unknown[] {
  if (path.endsWith(".json")) {
    try {
      return [JSON.parse(text)];
    } catch {
      throw new Error(`invalid canonical JSON: ${path}`);
    }
  }
  if (path.endsWith(".jsonl")) {
    return text
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  }
  return [];
}
function collectConversationRecords(
  value: unknown,
  output: Record<string, Record<string, unknown>>,
) {
  if (!value || typeof value !== "object") return;
  if (Array.isArray(value)) {
    for (const child of value) collectConversationRecords(child, output);
    return;
  }
  const record = value as Record<string, unknown>;
  const id = typeof record.id === "string" ? record.id : "";
  const agent = record.agent_id ?? record.agentId;
  if (
    /^conv-fake-headless-[1-9][0-9]*$/.test(id) &&
    typeof agent === "string"
  ) {
    output[id] = record;
  }
  for (const child of Object.values(record)) {
    collectConversationRecords(child, output);
  }
}
function enrichConversationRecords(
  value: unknown,
  records: Record<string, Record<string, unknown>>,
) {
  if (!value || typeof value !== "object") return;
  if (Array.isArray(value)) {
    for (const child of value) enrichConversationRecords(child, records);
    return;
  }
  const object = value as Record<string, unknown>;
  const context = metadataField(object, ["conversation_id", "conversationId"]);
  const candidates = [object.id, object.conversation_id, object.conversationId, context];
  for (const candidate of candidates) {
    if (typeof candidate !== "string" || !records[candidate]) continue;
    mergeCanonicalMetadata(records[candidate], object);
    const source = JSON.stringify(object).match(
      /Conversation ID[^:]+: (conv-fake-headless-[1-9][0-9]*)/,
    )?.[1];
    if (source && source !== candidate) records[candidate].source_id ??= source;
  }
  for (const child of Object.values(object)) {
    enrichConversationRecords(child, records);
  }
}
function mergeCanonicalMetadata(
  record: Record<string, unknown>, source: Record<string, unknown>,
) {
  const fields: Array<[string, string[]]> = [
    ["agent_id", ["agent_id", "agentId"]], ["hidden", ["hidden"]],
    ["source_id", ["openai_fork_source_conversation_id", "source_conversation_id", "source_id"]],
  ];
  for (const [target, names] of fields) {
    const value = metadataField(source, names);
    if (value !== undefined) record[target] = value;
  }
}
function metadataField(value: unknown, names: string[]): unknown {
  if (!value || typeof value !== "object") return undefined;
  if (Array.isArray(value)) {
    for (const child of value) {
      const found = metadataField(child, names);
      if (found !== undefined) return found;
    }
    return undefined;
  }
  const object = value as Record<string, unknown>;
  for (const name of names) {
    if (object[name] !== undefined) return object[name];
  }
  for (const child of Object.values(object)) {
    const found = metadataField(child, names);
    if (found !== undefined) return found;
  }
  return undefined;
}
function validateBackendOperations(
  records: Record<string, Record<string, unknown>>,
) {
  for (const [id, operation] of backendOperations) {
    const record = records[id];
    if (record) assertBackendOperation(record, id, operation);
  }
}
function assertBackendOperation(
  record: Record<string, unknown>,
  id: string,
  operation: BackendOperation,
) {
  if (String(operation.returned.id) !== id || String(record.id) !== id) {
    throw new Error(`backend/persisted conversation id divergence: ${id}`);
  }
  const agent = metadataField(record, ["agent_id", "agentId"]);
  const returnedAgent = operation.returned.agent_id ?? operation.returned.agentId;
  if (returnedAgent !== undefined && returnedAgent !== agent) {
    throw new Error(`backend/persisted agent divergence: ${id}`);
  }
  if (operation.agent !== undefined && operation.agent !== agent) {
    throw new Error(`requested/persisted agent divergence: ${id}`);
  }
  const hidden = metadataField(record, ["hidden"]);
  if (operation.hidden !== undefined && operation.hidden !== hidden) {
    throw new Error(`requested/persisted hidden divergence: ${id}`);
  }
  const source = metadataField(record, ["source_id"]);
  if (operation.kind === "fork" && operation.source !== source) {
    throw new Error(`called/persisted fork source divergence: ${id}`);
  }
}
function assertPersistenceMutationCoverage() {
  const id = "conv-fake-headless-1";
  const record = { id, agent_id: "agent-local-visible", hidden: true,
    source_id: "conv-fake-headless-2" };
  const base: BackendOperation = { kind: "fork", hidden: true,
    returned: { id, agent_id: "agent-local-visible" }, source: record.source_id };
  const mutations = [
    { ...base, returned: { ...base.returned, id: "conv-fake-headless-9" } },
    { ...base, source: "conv-fake-headless-3" },
  ];
  for (const mutation of mutations) {
    if (!rejects(() => assertBackendOperation(record, id, mutation)))
      throw new Error("persistence divergence mutation escaped");
  }
}
function durableSequence(values: unknown[], artifacts: string[]): number {
  let sequence = artifacts.reduce(
    (max, id) => Math.max(max, idSequence(id)),
    0,
  );
  const visit = (value: unknown, key = "") => {
    if (
      typeof value === "number" &&
      /conversation.*(sequence|counter)/i.test(key)
    ) {
      sequence = Math.max(sequence, value);
    } else if (Array.isArray(value)) {
      for (const child of value) visit(child, key);
    } else if (value && typeof value === "object") {
      for (const [name, child] of Object.entries(value)) visit(child, name);
    }
  };
  for (const value of values) visit(value);
  return sequence;
}
function idSequence(id: string): number {
  return Number(id.slice(id.lastIndexOf("-") + 1));
}
function sequenceOrder(left: string, right: string): number {
  return idSequence(left) - idSequence(right);
}
function canonicalProviderCall(value: any) {
  const inputs: string[] = [];
  for (const message of value.messages ?? []) {
    const parts = Array.isArray(message.content)
      ? message.content
      : [{ text: message.content }];
    for (const part of parts) {
      if (typeof part?.text === "string" && /^<[A-Z0-9_:]+>$/.test(part.text)) {
        inputs.push(part.text);
      }
    }
  }
  return {
    model: value.model,
    inputs,
    stream: value.stream === true,
    store: value.store === true,
  };
}
function observable(
  before: StoreSnapshot,
  after: StoreSnapshot,
  normalizer: Normalizer,
  phases: string[],
) {
  const ids = allocatedIds(before.sequence, after.sequence);
  const created = ids.map((id) => conversationLink(id, after.records[id]));
  const retained = created.filter((item) => after.records[item.id]);
  const deleted = ids.filter((id) => !after.artifacts.includes(id));
  const calls = providerCalls.slice(before.provider).map(canonicalProviderCall);
  return normalizer.value({
    provider_calls: calls,
    conversations: {
      created,
      deleted,
      retained,
      hidden: retained.filter((item) => item.hidden === true),
      forks: retained
        .filter((item) => item.source_id)
        .map((item) => ({ source_id: item.source_id, target_id: item.id })),
    },
    cleanup: { ephemeral_deleted: deleted.length },
    idempotency: {
      allocations: ids.length,
      admissions:
        retained.length > 0 || ids.length === 0
          ? roleDelta(before, after, "user")
          : 0,
      turns:
        retained.length > 0 || ids.length === 0
          ? roleDelta(before, after, "assistant")
          : 0,
      provider_calls: calls.length,
      live_joins: phases.includes("live_join") ? 1 : 0,
      phases,
    },
  });
}
function allocatedIds(before: number, after: number): string[] {
  if (after < before) throw new Error("conversation sequence regressed");
  return Array.from(
    { length: after - before },
    (_, index) => `conv-fake-headless-${before + index + 1}`,
  );
}
function conversationLink(id: string, record?: Record<string, unknown>) {
  return {
    id,
    agent_id: record?.agent_id ?? record?.agentId ?? null,
    hidden: typeof record?.hidden === "boolean" ? record.hidden : null,
    source_id:
      record?.openai_fork_source_conversation_id ??
      record?.source_conversation_id ??
      record?.source_id ??
      null,
  };
}
function roleDelta(before: StoreSnapshot, after: StoreSnapshot, role: string) {
  return Math.max(
    0,
    countRole(after.transcripts, role) - countRole(before.transcripts, role),
  );
}
function countRole(values: unknown[], role: string): number {
  let count = 0;
  const visit = (value: unknown) => {
    if (!value || typeof value !== "object") return;
    if (Array.isArray(value)) {
      for (const child of value) visit(child);
      return;
    }
    const record = value as Record<string, unknown>;
    if (record.role === role) count += 1;
    for (const child of Object.values(record)) visit(child);
  };
  for (const value of values) visit(value);
  return count;
}
type CaptureInput = {
  name: string;
  route: string;
  mode: string;
  dependencies?: string[];
  request: any;
  execution?: any;
};
type CaptureResult = {
  expected: unknown;
  cursor: unknown;
  phases: string[];
};
const cases: any[] = [];
let storedId: string | undefined;
let storedConversationId: string | undefined;
async function capture(input: CaptureInput) {
  const normalizer = new Normalizer();
  const before = await snapshot();
  const execution = input.execution ?? { kind: "single" };
  const result = await executeCapture(input, execution, normalizer, before);
  await Bun.sleep(25);
  const after = await snapshot();
  const body = input.request.body ?? {};
  const ephemeral =
    (input.route === "chat" &&
      input.request.headers?.["x-letta-chat-key"] === undefined) ||
    (input.route === "responses" && body.store !== true);
  assertHandlerDeleteContract(
    ephemeral,
    allocatedIds(before.sequence, after.sequence),
    after,
    backendDeleteCalls,
  );
  const observed = observable(before, after, normalizer, result.phases);
  cases.push({
    schema_version: 2,
    ...input,
    dependencies: input.dependencies ?? [],
    execution,
    expected: result.expected,
    observable: observed,
    relationships: normalizer.relationships(),
    cursor: result.cursor,
  });
}
async function executeCapture(
  input: CaptureInput,
  execution: any,
  normalizer: Normalizer,
  before: StoreSnapshot,
): Promise<CaptureResult> {
  if (execution.kind === "idempotent_live_join") {
    return executeLiveJoin(input, normalizer, before);
  }
  if (execution.kind === "repeat") {
    const outputs = [];
    for (let index = 0; index < execution.count; index++) {
      outputs.push(await send(input.request, normalizer));
    }
    return {
      expected: { attempts: outputs.map((output) => output.result) },
      cursor: null,
      phases: ["not_applicable"],
    };
  }
  return executeSingle(input, normalizer);
}
async function executeLiveJoin(
  input: CaptureInput,
  normalizer: Normalizer,
  before: StoreSnapshot,
): Promise<CaptureResult> {
  providerHeld = true;
  providerGate = new Promise<void>((resolve) => {
    providerRelease = resolve;
  });
  const arrived = new Promise<void>((resolve) => {
    providerArrived = resolve;
  });
  const firstResponse = await begin(input.request);
  const firstFinished = finish(firstResponse, input.mode, normalizer);
  await arrived;
  const owner = await snapshot();
  const received = baselineRequests + 1;
  const joinedPending = begin(input.request);
  await waitForBaselineRequest(received);
  const joined = await snapshot();
  assertSameStore(owner, joined, "live join");
  releaseProvider();
  const joinedResponse = await joinedPending;
  assertLiveJoin(joinedResponse, before);
  const joinedFinished = finish(joinedResponse, input.mode, normalizer);
  const first = await firstFinished;
  const second = await joinedFinished;
  const settledBefore = await snapshot();
  const replay = await send(input.request, normalizer);
  const settledAfter = await snapshot();
  assertSameStore(settledBefore, settledAfter, "settled replay");
  assertFreshRawResponseIdentities([first.raw, second.raw, replay.raw]);
  return {
    expected: { attempts: [first.result, second.result, replay.result] },
    cursor: null,
    phases: ["owner_active", "live_join", "settled_replay"],
  };
}
async function waitForBaselineRequest(expected: number) {
  for (let count = 0; count < 1_000; count++) {
    if (baselineRequests >= expected) return;
    await Bun.sleep(1);
  }
  throw new Error("joined HTTP request admission timeout");
}
function assertLiveJoin(response: Response, before: StoreSnapshot) {
  if (response.status !== 200 || providerCalls.length - before.provider !== 1) {
    throw new Error("second HTTP request was not an accepted live join");
  }
}
function releaseProvider() {
  providerHeld = false;
  providerRelease?.();
  providerRelease = null;
  providerGate = null;
}
async function executeSingle(
  input: CaptureInput,
  normalizer: Normalizer,
): Promise<CaptureResult> {
  const response = await begin(input.request, storedId);
  const raw = await response.clone().text();
  const rawBody = input.mode === "json" ? JSON.parse(raw) : null;
  const after = await snapshot();
  if (input.execution?.previous_cursor) recordPreviousFork(after);
  const output = await finish(response, input.mode, normalizer);
  const cursor = input.execution?.capture_cursor
    ? captureCursor(rawBody.id, after, normalizer)
    : null;
  return {
    expected: output.result,
    cursor,
    phases: ["not_applicable"],
  };
}
function recordPreviousFork(after: StoreSnapshot) {
  const record = Object.values(after.records)
    .sort((left, right) => sequenceOrder(String(left.id), String(right.id)))
    .at(-1);
  if (!record || record.hidden !== true || !storedConversationId) {
    throw new Error("previous response canonical hidden fork missing");
  }
  const target = String(record.id);
  const persistedSource = metadataField(record, ["source_id"]);
  const calledSource = rawForkSources[target];
  if (
    calledSource !== storedConversationId ||
    persistedSource !== storedConversationId
  ) {
    throw new Error("previous response raw/persisted cursor source mismatch");
  }
}
function captureCursor(
  id: string,
  store: StoreSnapshot,
  normalizer: Normalizer,
) {
  storedId = id;
  const encoded = id.slice("resp_letta_".length);
  const decoded = JSON.parse(
    Buffer.from(encoded, "base64url").toString("utf8"),
  );
  const fields = Object.keys(decoded).sort().join(",");
  if (
    fields !== "agent_id,conversation_id,nonce,version" ||
    decoded.version !== 1
  ) {
    throw new Error("cursor shape/version");
  }
  const record = store.records[decoded.conversation_id];
  const agent = record?.agent_id ?? record?.agentId;
  if (
    !record ||
    record.id !== decoded.conversation_id ||
    agent !== decoded.agent_id
  ) {
    throw new Error("cursor canonical agent/conversation mismatch");
  }
  const uuid =
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
  if (!uuid.test(decoded.nonce)) throw new Error("cursor nonce");
  storedConversationId = decoded.conversation_id;
  const value = normalizer.value(decoded);
  return value;
}
assertIdentityMutationCoverage();
assertTimestampMutationCoverage();
assertPersistenceMutationCoverage();
await capture({ name: "models_json", route: "models", mode: "json",
  request: request("GET", "/v1/models", "json") });
await capture({ name: "chat_headerless_json", route: "chat", mode: "json",
  request: request("POST", "/v1/chat/completions", "json", {
    model: "fixture-visible", messages: [{ role: "user", content: "<USER_TEXT_A>" }],
  }) });
await capture({ name: "chat_stream_sse", route: "chat", mode: "sse",
  request: request("POST", "/v1/chat/completions", "sse", {
    model: "agent-local-visible", stream: true,
    messages: [{ role: "user", content: "<USER_TEXT_B>" }],
  }) });
await capture({ name: "chat_stateful_first", route: "chat", mode: "json",
  request: request("POST", "/v1/chat/completions", "json", {
    model: "fixture-visible", messages: [{ role: "user", content: "<USER_TEXT_C>" }],
  }, { "x-letta-chat-key": "<CHAT_KEY>" }) });
await capture({ name: "chat_stateful_newest", route: "chat", mode: "json",
  dependencies: ["chat_stateful_first"],
  request: request("POST", "/v1/chat/completions", "json", {
    model: "fixture-visible", messages: [
      { role: "user", content: "<OLD_USER_TEXT>" },
      { role: "assistant", content: "<OLD_ASSISTANT_TEXT>" },
      { role: "user", content: "<NEWEST_USER_TEXT>" },
    ],
  }, { "x-letta-chat-key": "<CHAT_KEY>" }) });
await capture({ name: "chat_idempotent_retry", route: "chat", mode: "sse",
  execution: { kind: "idempotent_live_join" },
  request: request("POST", "/v1/chat/completions", "sse", {
    model: "fixture-visible", stream: true,
    messages: [{ role: "user", content: "<IDEMPOTENT_USER_TEXT>" }],
  }, { "idempotency-key": "<IDEMPOTENCY_KEY>",
    "x-letta-chat-key": "<IDEMPOTENCY_CHAT_KEY>" }) });
await capture({ name: "responses_nonstored_json", route: "responses", mode: "json",
  request: request("POST", "/v1/responses", "json", {
    model: "fixture-visible", input: "<RESPONSE_USER_TEXT_A>", store: false,
  }) });
await capture({ name: "responses_stream_sse", route: "responses", mode: "sse",
  request: request("POST", "/v1/responses", "sse", {
    model: "fixture-visible", input: "<RESPONSE_USER_TEXT_B>", stream: true,
  }) });
await capture({ name: "responses_stored_json", route: "responses", mode: "json",
  execution: { kind: "single", capture_cursor: true },
  request: request("POST", "/v1/responses", "json", {
    model: "fixture-visible", input: "<RESPONSE_USER_TEXT_C>", store: true,
  }) });
await capture({ name: "responses_previous_json", route: "responses", mode: "json",
  dependencies: ["responses_stored_json"],
  execution: { kind: "single", previous_cursor: "responses_stored_json" },
  request: request("POST", "/v1/responses", "json", {
    model: "fixture-visible", input: "<RESPONSE_USER_TEXT_D>", store: true,
    previous_response_id: "<FROM:responses_stored_json>",
  }) });
await capture({ name: "responses_no_idempotency", route: "responses", mode: "json",
  execution: { kind: "repeat", count: 2 },
  request: request(
    "POST",
    "/v1/responses",
    "json",
    { model: "fixture-visible", input: "<RESPONSE_USER_TEXT_E>", store: false },
    { "idempotency-key": "<IDEMPOTENCY_KEY>" },
  ),
});
const { closeOpenAiBridgeRuntime } =
  await import("@/websocket/app-server-openai-turn");
closeOpenAiBridgeRuntime();
await new Promise<void>((resolve) => baseline.close(() => resolve()));
await new Promise<void>((resolve) => provider.close(() => resolve()));
await Bun.write(Bun.stdout, JSON.stringify(cases));
