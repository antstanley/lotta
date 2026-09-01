import { createServer } from "node:http";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { mock } from "bun:test";

const TOKEN = "fixture-transport-credential";
const STORAGE = process.env.LETTA_LOCAL_BACKEND_DIR!;
const AGENTS = [
  { id: "agent-local-visible", name: "fixture-visible", created_at: "2026-01-01T00:00:00Z", hidden: false },
  { id: "agent-local-collision-a", name: "fixture-collision", created_at: "2026-01-02T00:00:00Z", hidden: false },
  { id: "agent-local-collision-b", name: "fixture-collision", created_at: "2026-01-03T00:00:00Z", hidden: false },
  { id: "agent-local-hidden", name: "fixture-hidden", created_at: "2026-01-04T00:00:00Z", hidden: true },
];
const state = {
  created: [] as Array<Record<string, unknown>>,
  deleted: [] as string[],
  forks: [] as Array<Record<string, unknown>>,
  admissions: 0,
  providerCalls: [] as unknown[],
  conversations: new Map<string, Record<string, unknown>>(),
};
let providerHeld = false;
let providerRelease: (() => void) | null = null;
let providerGate: Promise<void> | null = null;
let providerArrived: (() => void) | null = null;

function page<T>(items: T[]) {
  return { getPaginatedItems: () => items };
}

const provider = createServer(async (request, response) => {
  if (request.method === "GET") {
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ object: "list", data: [{ id: "default", object: "model" }] }));
    return;
  }
  const chunks: Buffer[] = [];
  for await (const chunk of request) chunks.push(Buffer.from(chunk));
  state.providerCalls.push(JSON.parse(Buffer.concat(chunks).toString("utf8")));
  providerArrived?.();
  if (providerHeld && providerGate) await providerGate;
  response.writeHead(200, { "content-type": "text/event-stream" });
  response.write('data: {"id":"chatcmpl-11111111-1111-4111-8111-111111111111","object":"chat.completion.chunk","created":1767225600,"model":"default","choices":[{"index":0,"delta":{"role":"assistant","content":"<ASSISTANT_TEXT>"},"finish_reason":null}]}\n\n');
  response.write('data: {"id":"chatcmpl-11111111-1111-4111-8111-111111111111","object":"chat.completion.chunk","created":1767225600,"model":"default","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}}\n\n');
  response.end("data: [DONE]\n\n");
});
await new Promise<void>((resolve) => provider.listen(0, "127.0.0.1", resolve));
const providerPort = (provider.address() as { port: number }).port;

await mkdir(join(STORAGE, "providers"), { recursive: true });
await writeFile(join(STORAGE, "providers/auth.json"), JSON.stringify({
  version: 1,
  providers: { "openai-compatible": {
    id: "local-provider-openai-compatible",
    name: "openai-compatible",
    provider_type: "openai-compatible",
    provider_category: "byok",
    auth: { type: "api", key: "fixture-provider-credential" },
    base_url: `http://127.0.0.1:${providerPort}/v1`,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  } },
}, null, 2));

const { HeadlessBackend } = await import("@/backend/dev/fake-headless-backend");
const { ProviderTurnExecutor } = await import("@/backend/dev/provider-turn-executor");
const { PiStreamAdapter } = await import("@/backend/dev/pi-stream-adapter");
const executor = new ProviderTurnExecutor(new PiStreamAdapter({ localProviderAuthStorageDir: STORAGE }));
const delegate = new HeadlessBackend(
  "agent-local-visible",
  executor,
  { storageDir: STORAGE, defaultAgentName: "fixture-visible", defaultAgentModel: "openai-compatible/default" },
  { modelHandle: "openai-compatible/default" },
);
await delegate.updateAgent("agent-local-visible", { name: "fixture-visible", system: "", model: "openai-compatible/default" } as never);
const backend = new Proxy(delegate as object, {
  get(target, property, receiver) {
    if (property === "listAgents") return async () => page(AGENTS.filter((agent) => !agent.hidden).sort((left, right) => left.id.localeCompare(right.id)));
    if (property === "retrieveAgent") return async (id: string) => {
      if (id === "agent-local-visible") return delegate.retrieveAgent(id);
      const agent = AGENTS.find((item) => item.id === id);
      if (!agent) throw new Error("not found");
      return agent;
    };
    if (property === "createConversation") return async (body: Record<string, unknown>) => {
      const value = await delegate.createConversation(body as never) as unknown as Record<string, unknown>;
      const record = { id: value.id, agent_id: body.agent_id, hidden: body.hidden === true, source_id: null };
      state.created.push(record);
      state.conversations.set(String(value.id), record);
      return value;
    };
    if (property === "deleteConversation") return async (id: string) => {
      state.deleted.push(id);
      state.conversations.delete(id);
      return (delegate as any).deleteConversation?.(id) ?? {};
    };
    if (property === "forkConversation") return async (id: string, options: Record<string, unknown> = {}) => {
      const value = await delegate.forkConversation(id, options as never) as unknown as Record<string, unknown>;
      const source = state.conversations.get(id);
      const record = { id: value.id, agent_id: value.agent_id ?? source?.agent_id ?? options.agentId, hidden: options.hidden === true, source_id: id };
      state.created.push(record);
      state.forks.push({ source_id: id, target_id: value.id });
      state.conversations.set(String(value.id), record);
      return value;
    };
    if (property === "createConversationMessageStream" || property === "streamConversationMessages") {
      return async (...args: unknown[]) => {
        state.admissions += 1;
        const [conversationId, body, ...rest] = args;
        const bounded = { ...(body as object), client_tools: [], client_skills: [] };
        return Reflect.get(target, property, receiver).apply(target, [conversationId, bounded, ...rest]);
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
const { handleOpenAiCompatRequest } = await import("@/websocket/app-server-openai");
const { parseAppServerWebsocketAuthSettings } = await import("@/websocket/app-server-auth");
const authPolicy = parseAppServerWebsocketAuthSettings({
  wsAuth: "capability-token",
  wsTokenSha256: "1947de502481746d5dc98a64e8fa1d743d6c3da164f5b031cbf9ee9e0fb05ffb",
});
const baseline = createServer((request, response) => {
  void handleOpenAiCompatRequest(request, response, { authPolicy });
});
await new Promise<void>((resolve) => baseline.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${(baseline.address() as { port: number }).port}`;

type DynamicKind = "CHAT_COMPLETION" | "STORED_RESPONSE" | "RESPONSE" | "MSG" | "FC" | "FCO" | "RS" | "UUID" | "CONVERSATION" | "TIMESTAMP";
class Normalizer {
  maps = new Map<DynamicKind, Map<string, string>>();
  token(kind: DynamicKind, original: string): string {
    let values = this.maps.get(kind);
    if (!values) { values = new Map(); this.maps.set(kind, values); }
    const present = values.get(original);
    if (present) return present;
    const token = `<${kind}_ID_${values.size + 1}>`;
    values.set(original, token);
    return token;
  }
  string(value: string): string {
    const uuid = "[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}";
    const patterns: Array<[DynamicKind, RegExp]> = [
      ["CHAT_COMPLETION", new RegExp(`^chatcmpl-${uuid}$`)],
      ["RESPONSE", new RegExp(`^resp_${uuid}$`)],
      ["MSG", new RegExp(`^msg_${uuid}$`)], ["FC", new RegExp(`^fc_${uuid}$`)],
      ["FCO", new RegExp(`^fco_${uuid}$`)], ["RS", new RegExp(`^rs_${uuid}$`)],
      ["UUID", new RegExp(`^${uuid}$`)],
      ["CONVERSATION", /^conv-fake-headless-[1-9][0-9]*$/],
    ];
    if (value.startsWith("resp_letta_")) return this.token("STORED_RESPONSE", value);
    for (const [kind, pattern] of patterns) if (pattern.test(value)) return this.token(kind, value);
    if (/^(chatcmpl-|resp_|msg_|fc_|fco_|rs_|conv-fake-headless-)/.test(value)) {
      throw new Error(`malformed dynamic identifier: ${value}`);
    }
    return value;
  }
  value(value: unknown, key = ""): unknown {
    if (typeof value === "number" && (key === "created" || key === "created_at" || key === "timestamp")) {
      return this.token("TIMESTAMP", String(value));
    }
    if (typeof value === "string") return this.string(value);
    if (Array.isArray(value)) return value.map((child) => this.value(child, key));
    if (value && typeof value === "object") {
      return Object.fromEntries(Object.entries(value).map(([name, child]) => [name, this.value(child, name)]));
    }
    return value;
  }
  relationships(): Record<string, string[]> {
    return Object.fromEntries([...this.maps].map(([kind, values]) => [kind.toLowerCase(), [...values.values()]]));
  }
}

function headers(source: Headers) {
  const result: Record<string, string> = {};
  for (const name of ["content-type", "cache-control"]) {
    const value = source.get(name);
    if (value) result[name] = value;
  }
  return result;
}
function parseSse(raw: string, normalizer: Normalizer) {
  return raw.split("\n\n").filter(Boolean).map((block) => {
    let event: string | null = null;
    let data: string | null = null;
    for (const line of block.split("\n")) {
      if (line.startsWith("event: ")) event = line.slice(7);
      else if (line.startsWith("data: ")) data = line.slice(6);
      else throw new Error(`invalid SSE framing: ${line}`);
    }
    if (data === null) throw new Error("SSE block missing data");
    return { event, data: data === "[DONE]" ? data : normalizer.value(JSON.parse(data)) };
  });
}
function materialize(request: any, previousId?: string) {
  const body = structuredClone(request.body);
  if (body?.previous_response_id === "<FROM:responses_stored_json>") body.previous_response_id = previousId;
  return body;
}
async function begin(request: any, previousId?: string) {
  const requestHeaders: Record<string, string> = { "content-type": "application/json", authorization: `Bearer ${TOKEN}` };
  for (const [name, value] of Object.entries(request.headers ?? {})) if (name !== "authorization") requestHeaders[name] = String(value);
  return fetch(origin + request.path, {
    method: request.method, headers: requestHeaders,
    body: request.body ? JSON.stringify(materialize(request, previousId)) : undefined,
  });
}
async function finish(response: Response, mode: string, _caseNormalizer: Normalizer) {
  const raw = await response.text();
  const normalizer = new Normalizer();
  const result: Record<string, unknown> = { status: response.status, headers: headers(response.headers) };
  if (mode === "sse") result.events = parseSse(raw, normalizer);
  else result.body = normalizer.value(JSON.parse(raw));
  result.dynamic_map = normalizer.relationships();
  return { result, raw };
}
async function send(request: any, normalizer: Normalizer, previousId?: string) {
  return finish(await begin(request, previousId), request.mode, normalizer);
}
function request(method: string, path: string, mode: string, body: unknown = null, extra: Record<string, string> = {}) {
  return { method, path, mode, headers: { authorization: "<AUTHORIZATION>", ...extra }, body };
}
function snapshot() {
  return { created: state.created.length, deleted: state.deleted.length, forks: state.forks.length, admissions: state.admissions, provider: state.providerCalls.length };
}
function canonicalProviderCall(value: any) {
  const inputs: string[] = [];
  for (const message of value.messages ?? []) {
    const parts = Array.isArray(message.content) ? message.content : [{ text: message.content }];
    for (const part of parts) if (typeof part?.text === "string" && /^<[A-Z0-9_:]+>$/.test(part.text)) inputs.push(part.text);
  }
  return { model: value.model, inputs, stream: value.stream === true, store: value.store === true };
}
function observable(before: ReturnType<typeof snapshot>, normalizer: Normalizer, idempotency: unknown) {
  const created = state.created.slice(before.created);
  const deleted = state.deleted.slice(before.deleted);
  const forks = state.forks.slice(before.forks);
  return normalizer.value({
    provider_calls: state.providerCalls.slice(before.provider).map(canonicalProviderCall),
    conversations: { created, deleted, hidden: created.filter((item) => item.hidden), forks },
    cleanup: { ephemeral_deleted: deleted.length },
    idempotency: idempotency ?? { allocations: created.length, admissions: state.admissions - before.admissions, provider_calls: state.providerCalls.length - before.provider, live_joins: 0, phases: ["not_applicable"] },
  });
}

const cases: any[] = [];
let storedId: string | undefined;
let storedConversationId: string | undefined;
async function capture(input: { name: string; route: string; mode: string; dependencies?: string[]; request: any; execution?: any }) {
  const normalizer = new Normalizer();
  const before = snapshot();
  let expected: unknown;
  let cursor: unknown = null;
  let idempotency: unknown = null;
  const execution = input.execution ?? { kind: "single" };
  if (execution.kind === "idempotent_live_join") {
    providerHeld = true;
    providerGate = new Promise<void>((resolve) => { providerRelease = resolve; });
    const arrived = new Promise<void>((resolve) => { providerArrived = resolve; });
    const firstResponse = await begin(input.request);
    const firstFinished = finish(firstResponse, input.mode, normalizer);
    await arrived;
    const joinedResponse = await begin(input.request);
    if (joinedResponse.status !== 200 || state.providerCalls.length - before.provider !== 1) throw new Error("live join barrier failed");
    const joinedFinished = finish(joinedResponse, input.mode, normalizer);
    providerHeld = false; providerRelease?.(); providerRelease = null; providerGate = null;
    const first = await firstFinished;
    const joined = await joinedFinished;
    const replay = await send(input.request, normalizer);
    expected = { attempts: [first.result, joined.result, replay.result] };
    idempotency = { allocations: 1, admissions: 1, provider_calls: 1, live_joins: 1, phases: ["owner_active", "live_join", "settled_replay"] };
  } else if (execution.kind === "repeat") {
    const attempts = [];
    for (let index = 0; index < execution.count; index++) attempts.push((await send(input.request, normalizer)).result);
    expected = { attempts };
  } else {
    const output = await send(input.request, normalizer, storedId);
    expected = output.result;
    const rawBody = input.mode === "json" ? JSON.parse(output.raw) : null;
    if (input.execution?.previous_cursor) {
      const fork = state.forks.at(-1);
      const target = fork ? state.conversations.get(String(fork.target_id)) : null;
      if (!fork || fork.source_id !== storedConversationId || target?.hidden !== true) {
        throw new Error("previous response hidden fork source mismatch");
      }
    }
    if (input.execution?.capture_cursor) {
      storedId = rawBody.id;
      const encoded = storedId!.slice("resp_letta_".length);
      const decoded = JSON.parse(Buffer.from(encoded, "base64url").toString("utf8"));
      if (Object.keys(decoded).sort().join(",") !== "agent_id,conversation_id,nonce,version") throw new Error("cursor fields");
      if (decoded.version !== 1 || decoded.agent_id !== "agent-local-visible") throw new Error("cursor version/agent");
      const record = state.conversations.get(decoded.conversation_id);
      if (!record || record.id !== decoded.conversation_id || record.agent_id !== decoded.agent_id) {
        throw new Error("cursor canonical conversation");
      }
      if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(decoded.nonce)) throw new Error("cursor nonce");
      storedConversationId = decoded.conversation_id;
      cursor = normalizer.value(decoded);
    }
  }
  cases.push({ schema_version: 2, ...input, dependencies: input.dependencies ?? [], execution, expected,
    observable: observable(before, normalizer, idempotency), relationships: normalizer.relationships(), cursor });
}

await capture({ name: "models_json", route: "models", mode: "json", request: request("GET", "/v1/models", "json") });
await capture({ name: "chat_headerless_json", route: "chat", mode: "json", request: request("POST", "/v1/chat/completions", "json", { model: "fixture-visible", messages: [{ role: "user", content: "<USER_TEXT_A>" }] }) });
await capture({ name: "chat_stream_sse", route: "chat", mode: "sse", request: request("POST", "/v1/chat/completions", "sse", { model: "agent-local-visible", messages: [{ role: "user", content: "<USER_TEXT_B>" }], stream: true }) });
await capture({ name: "chat_stateful_first", route: "chat", mode: "json", request: request("POST", "/v1/chat/completions", "json", { model: "fixture-visible", messages: [{ role: "user", content: "<USER_TEXT_C>" }] }, { "x-letta-chat-key": "<CHAT_KEY>" }) });
await capture({ name: "chat_stateful_newest", route: "chat", mode: "json", dependencies: ["chat_stateful_first"], request: request("POST", "/v1/chat/completions", "json", { model: "fixture-visible", messages: [{ role: "user", content: "<OLD_USER_TEXT>" }, { role: "assistant", content: "<OLD_ASSISTANT_TEXT>" }, { role: "user", content: "<NEWEST_USER_TEXT>" }] }, { "x-letta-chat-key": "<CHAT_KEY>" }) });
await capture({ name: "chat_idempotent_retry", route: "chat", mode: "sse", execution: { kind: "idempotent_live_join" }, request: request("POST", "/v1/chat/completions", "sse", { model: "fixture-visible", messages: [{ role: "user", content: "<IDEMPOTENT_USER_TEXT>" }], stream: true }, { "idempotency-key": "<IDEMPOTENCY_KEY>" }) });
await capture({ name: "responses_nonstored_json", route: "responses", mode: "json", request: request("POST", "/v1/responses", "json", { model: "fixture-visible", input: "<RESPONSE_USER_TEXT_A>", store: false }) });
await capture({ name: "responses_stream_sse", route: "responses", mode: "sse", request: request("POST", "/v1/responses", "sse", { model: "fixture-visible", input: "<RESPONSE_USER_TEXT_B>", stream: true }) });
await capture({ name: "responses_stored_json", route: "responses", mode: "json", execution: { kind: "single", capture_cursor: true }, request: request("POST", "/v1/responses", "json", { model: "fixture-visible", input: "<RESPONSE_USER_TEXT_C>", store: true }) });
await capture({ name: "responses_previous_json", route: "responses", mode: "json", dependencies: ["responses_stored_json"], execution: { kind: "single", previous_cursor: "responses_stored_json" }, request: request("POST", "/v1/responses", "json", { model: "fixture-visible", input: "<RESPONSE_USER_TEXT_D>", previous_response_id: "<FROM:responses_stored_json>", store: true }) });
await capture({ name: "responses_no_idempotency", route: "responses", mode: "json", execution: { kind: "repeat", count: 2 }, request: request("POST", "/v1/responses", "json", { model: "fixture-visible", input: "<RESPONSE_USER_TEXT_E>", store: false }, { "idempotency-key": "<IDEMPOTENCY_KEY>" }) });

const { closeOpenAiBridgeRuntime } = await import("@/websocket/app-server-openai-turn");
closeOpenAiBridgeRuntime();
await new Promise<void>((resolve) => baseline.close(() => resolve()));
await new Promise<void>((resolve) => provider.close(() => resolve()));
await Bun.write(Bun.stdout, JSON.stringify(cases));
