#!/usr/bin/env node
import { createHash } from "node:crypto";
import { chmod, mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import process from "node:process";

const BASELINE_COMMIT = "300f923f16cc8eee50656d7da732902c1dea2b65";
const CASES_MAX = 16;
const REQUIRED_CASES = [
  "models_json",
  "chat_headerless_json",
  "chat_stream_sse",
  "chat_stateful_first",
  "chat_stateful_newest",
  "chat_idempotent_retry",
  "responses_nonstored_json",
  "responses_stream_sse",
  "responses_stored_json",
  "responses_previous_json",
  "responses_no_idempotency",
];
const FIXTURE_BYTES_MAX = 1_048_576;
const COMMAND_TIMEOUT_MS = 300_000;
const repository = process.env.LETTA_CODE_REPOSITORY ??
  "/Volumes/Delorean/code/five-letters/letta-code";
const output = resolve(process.argv[2] ?? new URL("../fixtures/openai", import.meta.url).pathname);

function run(command, args, options = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, { ...options, stdio: ["ignore", "pipe", "pipe"] });
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`command timed out: ${command}`));
    }, COMMAND_TIMEOUT_MS);
    const stdout = [];
    const stderr = [];
    child.stdout.on("data", (chunk) => stdout.push(chunk));
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      const out = Buffer.concat(stdout).toString("utf8");
      const err = Buffer.concat(stderr).toString("utf8");
      if (code !== 0) reject(new Error(`${command} exited ${code}: ${err}`));
      else resolvePromise(out);
    });
  });
}

const runner = String.raw`
import { createServer } from "node:http";
import { mock } from "bun:test";

const TOKEN = "fixture-transport-credential";
const AGENTS = [
  { id: "agent-local-visible", name: "fixture-visible", created_at: "2026-01-01T00:00:00Z", hidden: false },
  { id: "agent-local-collision-a", name: "fixture-collision", created_at: "2026-01-02T00:00:00Z", hidden: false },
  { id: "agent-local-collision-b", name: "fixture-collision", created_at: "2026-01-03T00:00:00Z", hidden: false },
  { id: "agent-local-hidden", name: "fixture-hidden", created_at: "2026-01-04T00:00:00Z", hidden: true },
];
const state = { conversations: new Map(), created: [], deleted: [], forked: [], providerCalls: [], next: 1 };
const backend = {
  listAgents: async () => AGENTS.filter((agent) => !agent.hidden).sort((left, right) => left.id.localeCompare(right.id)),
  createConversation: async ({ agent_id, hidden = false }) => {
    const id = "conversation-fixture-" + state.next++;
    state.created.push(id);
    state.conversations.set(id, { id, agent_id, hidden, messages: [] });
    return { id, agent_id, hidden };
  },
  deleteConversation: async (id) => { state.deleted.push(id); state.conversations.delete(id); return {}; },
  retrieveConversation: async (id) => {
    const value = state.conversations.get(id);
    if (!value) throw new Error("not found");
    return value;
  },
  forkConversation: async (id, options = {}) => {
    const source = state.conversations.get(id);
    if (!source) throw new Error("not found");
    const next = "conversation-fixture-" + state.next++;
    const value = { ...source, id: next, hidden: options.hidden === true, messages: [...source.messages] };
    state.conversations.set(next, value);
    state.created.push(next);
    state.forked.push({ source: id, target: next, hidden: value.hidden });
    return value;
  },
};
let runTurnImpl;
mock.module("@/backend", () => ({ getBackend: () => backend }));
mock.module("@/websocket/app-server-openai-turn", () => ({
  runBridgeTurn: (...args) => runTurnImpl(...args),
  __testSetRunTurnImpl: (value) => { runTurnImpl = value; },
}));

const provider = createServer(async (request, response) => {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  state.providerCalls.push(body);
  await new Promise((done) => setTimeout(done, 60));
  response.writeHead(200, { "content-type": "text/event-stream" });
  response.write('data: {"choices":[{"index":0,"delta":{"content":"<ASSISTANT_TEXT>"},"finish_reason":null}]}\n\n');
  response.write('data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":0,\"total_tokens\":0}}\n\n');
  response.end("data: [DONE]\n\n");
});
await new Promise((done) => provider.listen(0, "127.0.0.1", done));
const providerPort = provider.address().port;

runTurnImpl = async ({ conversationId, messages, onAssistantText }) => {
  const conversation = state.conversations.get(conversationId);
  if (conversation) conversation.messages.push(...messages);
  const response = await fetch("http://127.0.0.1:" + providerPort + "/v1/chat/completions", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ model: "fixture-model", messages, stream: true }),
  });
  const text = await response.text();
  let answer = "";
  let usage = { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 };
  for (const block of text.split("\n\n")) {
    if (!block.startsWith("data: ") || block === "data: [DONE]") continue;
    const value = JSON.parse(block.slice(6));
    const delta = value.choices?.[0]?.delta?.content;
    if (delta) { answer += delta; onAssistantText?.(delta); }
    if (value.usage) usage = value.usage;
  }
  return { text: answer, usage, error: null };
};

const { handleOpenAiCompatRequest } = await import("@/websocket/app-server-openai");
const { parseAppServerWebsocketAuthSettings } = await import("@/websocket/app-server-auth");
const authPolicy = parseAppServerWebsocketAuthSettings({
  wsAuth: "capability-token",
  wsTokenSha256: "1947de502481746d5dc98a64e8fa1d743d6c3da164f5b031cbf9ee9e0fb05ffb",
});
const baseline = createServer((request, response) => {
  void handleOpenAiCompatRequest(request, response, { authPolicy });
});
await new Promise((done) => baseline.listen(0, "127.0.0.1", done));
const origin = "http://127.0.0.1:" + baseline.address().port;

function normalizedHeaders(headers) {
  const result = {};
  for (const name of ["content-type", "cache-control"]) {
    const value = headers.get(name);
    if (value) result[name] = value;
  }
  return result;
}
function dynamic(value, context = "") {
  if (typeof value === "number" && (context === "created" || context === "created_at")) return "<UNIX_TIMESTAMP>";
  if (typeof value === "string") {
    if (value.startsWith("chatcmpl-")) return "<CHAT_COMPLETION_ID>";
    if (value.startsWith("resp_letta_")) return "<STORED_RESPONSE_ID>";
    if (/^resp_[0-9a-f-]+$/i.test(value)) return "<RESPONSE_ID>";
    if (/^(msg|fc|fco|rs)_[0-9a-f-]+$/i.test(value)) return "<" + value.split("_")[0].toUpperCase() + "_ID>";
    if (/^[0-9a-f]{8}-[0-9a-f-]{27}$/i.test(value)) return "<UUID>";
    if (value.startsWith("conversation-fixture-")) return "<CONVERSATION_ID>";
  }
  if (Array.isArray(value)) return value.map((item) => dynamic(item, context));
  if (value && typeof value === "object") {
    const next = {};
    for (const [key, child] of Object.entries(value)) next[key] = dynamic(child, key);
    return next;
  }
  return value;
}
function parseSse(raw) {
  const events = [];
  for (const block of raw.split("\n\n")) {
    if (!block) continue;
    const lines = block.split("\n");
    let event = null;
    let data = null;
    for (const line of lines) {
      if (line.startsWith("event: ")) event = line.slice(7);
      else if (line.startsWith("data: ")) data = line.slice(6);
      else throw new Error("invalid SSE framing: " + line);
    }
    if (data === null) throw new Error("SSE block missing data");
    events.push({ event, data: data === "[DONE]" ? "[DONE]" : dynamic(JSON.parse(data)) });
  }
  return events;
}
async function send(request, previousId = null) {
  const headers = { "content-type": "application/json", authorization: "Bearer " + TOKEN };
  for (const [name, value] of Object.entries(request.headers ?? {})) {
    if (name !== "authorization") headers[name] = value;
  }
  let body = request.body;
  if (previousId && body?.previous_response_id === "<FROM:responses_stored_json>") {
    body = { ...body, previous_response_id: previousId };
  }
  const response = await fetch(origin + request.path, { method: request.method, headers, body: body ? JSON.stringify(body) : undefined });
  const raw = await response.text();
  return {
    status: response.status,
    headers: normalizedHeaders(response.headers),
    body: request.mode === "sse" ? undefined : dynamic(JSON.parse(raw)),
    events: request.mode === "sse" ? parseSse(raw) : undefined,
    raw,
  };
}
function request(method, path, mode, body = null, headers = {}) {
  return { method, path, mode, headers: { authorization: "<AUTHORIZATION>", ...headers }, body };
}
function observable(before) {
  const conversations = [...state.conversations.values()].map((value) => ({
    id: dynamic(value.id), agent_id: value.agent_id, hidden: value.hidden,
    messages: dynamic(value.messages),
  }));
  return dynamic({ provider_calls_delta: state.providerCalls.length - before.provider,
    created_delta: state.created.length - before.created,
    deleted_delta: state.deleted.length - before.deleted,
    forked_delta: state.forked.length - before.forked,
    conversations });
}
function snapshot() { return { provider: state.providerCalls.length, created: state.created.length, deleted: state.deleted.length, forked: state.forked.length }; }
const cases = [];
function fixtureExpected(value) {
  if (Array.isArray(value)) return value.map(fixtureExpected);
  if (value && typeof value === "object") {
    const next = {};
    for (const [key, child] of Object.entries(value)) {
      if (key !== "raw" && key !== "actualId" && key !== "cursor") next[key] = fixtureExpected(child);
    }
    return next;
  }
  return value;
}
async function capture(name, route, mode, dependencies, req, execute) {
  const before = snapshot();
  const result = execute ? await execute(req) : await send(req);
  cases.push({ schema_version: 1, name, route, mode, dependencies, request: req,
    expected: fixtureExpected(result.expected ?? result), observable: observable(before), cursor: result.cursor });
  return result;
}

await capture("models_json", "models", "json", [], request("GET", "/v1/models", "json"));
await capture("chat_headerless_json", "chat", "json", [], request("POST", "/v1/chat/completions", "json", {
  model: "fixture-visible", messages: [{ role: "user", content: "<USER_TEXT_A>" }],
}));
await capture("chat_stream_sse", "chat", "sse", [], request("POST", "/v1/chat/completions", "sse", {
  model: "agent-local-visible", messages: [{ role: "user", content: "<USER_TEXT_B>" }], stream: true,
}));
await capture("chat_stateful_first", "chat", "json", [], request("POST", "/v1/chat/completions", "json", {
  model: "fixture-visible", messages: [{ role: "user", content: "<USER_TEXT_C>" }],
}, { "x-letta-chat-key": "<CHAT_KEY>" }));
await capture("chat_stateful_newest", "chat", "json", ["chat_stateful_first"], request("POST", "/v1/chat/completions", "json", {
  model: "fixture-visible", messages: [
    { role: "user", content: "<OLD_USER_TEXT>" },
    { role: "assistant", content: "<OLD_ASSISTANT_TEXT>" },
    { role: "user", content: "<NEWEST_USER_TEXT>" },
  ],
}, { "x-letta-chat-key": "<CHAT_KEY>" }));
await capture("chat_idempotent_retry", "chat", "json", [], request("POST", "/v1/chat/completions", "json", {
  model: "fixture-visible", messages: [{ role: "user", content: "<IDEMPOTENT_USER_TEXT>" }],
}, { "idempotency-key": "<IDEMPOTENCY_KEY>" }), async (req) => {
  const firstPromise = send(req);
  await new Promise((done) => setTimeout(done, 10));
  const joinedPromise = send(req);
  const [first, joined] = await Promise.all([firstPromise, joinedPromise]);
  const replay = await send({ ...req, headers: { authorization: "<AUTHORIZATION>", "x-idempotency-key": "<IDEMPOTENCY_KEY>" } });
  return { expected: { attempts: [first, joined, replay] } };
});
await capture("responses_nonstored_json", "responses", "json", [], request("POST", "/v1/responses", "json", {
  model: "fixture-visible", input: "<RESPONSE_USER_TEXT_A>", store: false,
}));
await capture("responses_stream_sse", "responses", "sse", [], request("POST", "/v1/responses", "sse", {
  model: "fixture-visible", input: "<RESPONSE_USER_TEXT_B>", stream: true,
}));
const stored = await capture("responses_stored_json", "responses", "json", [], request("POST", "/v1/responses", "json", {
  model: "fixture-visible", input: "<RESPONSE_USER_TEXT_C>", store: true,
}), async (req) => {
  const raw = await send(req);
  const id = JSON.parse(raw.raw).id;
  const encoded = id.slice("resp_letta_".length);
  const decoded = JSON.parse(Buffer.from(encoded, "base64url").toString("utf8"));
  if (decoded.version !== 1 || !decoded.nonce || !decoded.agent_id || !decoded.conversation_id) throw new Error("invalid four-field cursor");
  return { ...raw, actualId: id, cursor: dynamic(decoded) };
});
await capture("responses_previous_json", "responses", "json", ["responses_stored_json"], request("POST", "/v1/responses", "json", {
  model: "fixture-visible", input: "<RESPONSE_USER_TEXT_D>", previous_response_id: "<FROM:responses_stored_json>", store: true,
}), async (req) => send(req, stored.actualId));
await capture("responses_no_idempotency", "responses", "json", [], request("POST", "/v1/responses", "json", {
  model: "fixture-visible", input: "<RESPONSE_USER_TEXT_E>", store: false,
}, { "idempotency-key": "<IDEMPOTENCY_KEY>" }), async (req) => {
  const first = await send(req); const second = await send(req);
  return { expected: { attempts: [first, second] } };
});

await new Promise((done) => baseline.close(done));
await new Promise((done) => provider.close(done));
console.log(JSON.stringify(cases));
`;

async function main() {
  const resolved = (await run("git", ["-C", repository, "rev-parse", `${BASELINE_COMMIT}^{commit}`])).trim();
  if (resolved !== BASELINE_COMMIT) throw new Error(`baseline mismatch: ${resolved}`);
  const archiveHash = (await run("git", ["-C", repository, "show", "-s", "--format=%T", BASELINE_COMMIT])).trim();
  if (!/^[0-9a-f]{40}$/.test(archiveHash)) throw new Error("baseline tree hash missing");
  const temp = await mkdtemp(join(tmpdir(), "lotta-openai-capture-"));
  try {
    const archive = join(temp, "baseline.tar");
    await run("git", ["-C", repository, "archive", "--format=tar", "-o", archive, BASELINE_COMMIT]);
    const tree = join(temp, "tree");
    await mkdir(tree);
    await run("tar", ["-xf", archive, "-C", tree]);
    const bun = process.env.LOTTA_BUN ?? "/Users/stan/.bun/bin/bun";
    await writeFile(join(tree, "task77-capture.ts"), runner);
    const stdout = await run(bun, ["task77-capture.ts"], {
      cwd: tree,
      env: { ...process.env, HOME: join(temp, "home"), LETTA_LOCAL_BACKEND_DIR: join(temp, "store") },
    });
    const cases = JSON.parse(stdout.trim());
    if (!Array.isArray(cases) || cases.length === 0 || cases.length > CASES_MAX) throw new Error("capture cases missing or over bound");
    const names = cases.map((fixture) => fixture.name);
    if (names.length !== REQUIRED_CASES.length || REQUIRED_CASES.some((name) => !names.includes(name))) {
      throw new Error("required baseline capture case missing");
    }
    await rm(output, { recursive: true, force: true });
    await mkdir(join(output, "cases"), { recursive: true });
    const entries = [];
    for (const fixture of cases) {
      if (!fixture.name || !fixture.route || !fixture.mode) throw new Error("captured case incomplete");
      const text = `${JSON.stringify(fixture, null, 2)}\n`;
      if (Buffer.byteLength(text) > FIXTURE_BYTES_MAX) throw new Error(`fixture too large: ${fixture.name}`);
      const path = `cases/${fixture.name}.json`;
      await writeFile(join(output, path), text);
      entries.push({ name: fixture.name, route: fixture.route, mode: fixture.mode,
        dependencies: fixture.dependencies, path, sha256: createHash("sha256").update(text).digest("hex") });
    }
    const index = { schema_version: 1, baseline_commit: BASELINE_COMMIT, baseline_tree: archiveHash,
      capture_command: "LOTTA_BUN=bun node tools/capture-openai-fixtures.mjs",
      bounds: { cases_max: CASES_MAX, fixture_bytes_max: FIXTURE_BYTES_MAX, events_per_case_max: 64, json_depth_max: 32 },
      cases: entries };
    await writeFile(join(output, "index.json"), `${JSON.stringify(index, null, 2)}\n`);
    await chmod(join(output, "index.json"), 0o644);
    console.log(`captured ${entries.length} cases from ${BASELINE_COMMIT}`);
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
}

await main();
