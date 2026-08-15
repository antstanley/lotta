#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, renameSync,
  rmSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const PIN = "300f923f16cc8eee50656d7da732902c1dea2b65";
const PI_AI_VERSION = "0.82.1";
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = resolve(ROOT, "fixtures/providers");
const LIMITS = Object.freeze({
  files: 80,
  bytes: 1048576,
  totalBytes: 4194304,
  depth: 4,
  name: 96,
});
const DIALECTS = Object.freeze([
  "openai-compatible", "anthropic", "ollama", "lm-studio", "llama-cpp",
]);
const DIMENSIONS = Object.freeze([
  "request_mapping", "event_order", "tool_call_assembly", "usage",
  "cancellation", "errors", "timeout", "context_overflow", "retry_after",
  "image_policy",
]);
const SOURCE_HASHES = Object.freeze({
  "src/backend/dev/pi-api-streams.ts":
    "fa9f9938028365ab1ccc8dffaff20c5f50a4214955f863098da9a5fd460e2155",
  "src/backend/dev/pi-stream-adapter.ts":
    "44d5eaa863a46574bfe9fc70cc196b7cf10c4b92fcd3674e43934195de17acdc",
  "src/backend/dev/pi-image-elision.ts":
    "88bd72460558ec21bed858f161b6c803a120b1cf66c2ca43169f6fbb0b920cc7",
  "src/backend/dev/provider-turn-executor.ts":
    "c94adaa8d09dc8769ec3299d461c2dae629491d75f9f074e3d12fd2f66985eff",
  "src/backend/dev/local-provider-errors.ts":
    "ccb9edc5260829ba9bf2139bb52157db4fe0cd4e432a1ca2c0ddf5a709c88f11",
  "src/backend/local/local-stream-chunks.ts":
    "663a49c1495813130c488704ed508f3f996f7c578dbb3f4031a08ece5875ef7a",
  "src/backend/dev/pi-provider-registry.ts":
    "f9f8d4ed4671b010c0d14a988d9a297ec2ea050eb8dbc43cefe8d4503c87f6ef",
  "src/backend/dev/pi-ollama-provider.ts":
    "d9a5e4d84df2f5037114db35983d4c3b1ab9f83df8b5c6798c1afada88dac983",
  "src/backend/dev/pi-llama-cpp-provider.ts":
    "41f3e112fafc645e0278d46a6e1bf3859eddf5328906293c11ebd613a58bb6b3",
  "src/backend/dev/pi-openai-compatible-provider.ts":
    "2c361268ddfb4dce5178c80d707301e4e59eacd1706aa126866ced8780bb5b0a",
  "src/backend/dev/pi-local-endpoint-provider.ts":
    "51420b6ae8dee8b20c73f3bf29b6577195fce167bff933e32fa10381831fd828",
  "src/backend/dev/pi-lmstudio-provider.ts":
    "40f244566e2c961f5f01e586324e0af7e8dee3c4c857a578e7112ee60fd6336a",
  "src/backend/pi-stream-adapter.test.ts":
    "9c7966327a9d7f0752d73a28c15fb6462574f36124528cdce7daff0522348302",
  "src/backend/pi-stream-adapter-context-pressure.test.ts":
    "dcc8f0983a7d617cd880c8cb090fa0b543f8d92bdbf67e42b7b33c629b0664de",
  "src/backend/pi-stream-adapter-local-endpoint.test.ts":
    "95f161e71fa4222f462b48364b0628d57e739af99c79b9eb2285a15a60f41daf",
  "src/backend/provider-turn-executor.test.ts":
    "4c0156acf7e0221cfa8ba889f91e5d2035918392289f4d1f62eb6133f1e025b2",
  "src/backend/local-provider-errors.test.ts":
    "ececd0406b1f3d598d822fdf38cc5f9a4e282de6dfd55bcb07de62a31a273f54",
  "bun.lock": "0a8cad33168b97cfd08958d29b853f683eff9a36f42d1709e761990a74adc26e",
});
const REGION_NAMES = Object.freeze({
  "src/backend/dev/pi-api-streams.ts": ["knownApiStreams"],
  "src/backend/dev/pi-stream-adapter.ts": ["toPiMessage", "toPiTools", "PiStreamAdapter"],
  "src/backend/dev/pi-image-elision.ts": ["elideImagePayloadsForProviderRetry"],
  "src/backend/dev/provider-turn-executor.ts": ["providerStreamPart", "createProviderLettaStream"],
  "src/backend/dev/local-provider-errors.ts": [
    "normalizeLocalProviderError",
    "localProviderRetryDelayMs",
  ],
  "src/backend/local/local-stream-chunks.ts": ["attachLocalMessage"],
  "src/backend/dev/pi-provider-registry.ts": ["builtinCatalogModels"],
  "src/backend/dev/pi-ollama-provider.ts": ["ollamaDiscover", "createOllamaPiProvider"],
  "src/backend/dev/pi-llama-cpp-provider.ts": ["llamaCppDiscover", "createLlamaCppPiProvider"],
  "src/backend/dev/pi-openai-compatible-provider.ts": [
    "openAICompatibleDiscover",
    "createOpenAICompatiblePiProvider",
  ],
  "src/backend/dev/pi-local-endpoint-provider.ts": ["createLocalEndpointPiProvider"],
  "src/backend/dev/pi-lmstudio-provider.ts": ["lmStudioDiscover", "createLmStudioPiProvider"],
  "src/backend/pi-stream-adapter.test.ts": ["streamFromEvents"],
  "src/backend/pi-stream-adapter-context-pressure.test.ts": ["streamFromMessage"],
  "src/backend/pi-stream-adapter-local-endpoint.test.ts": ["input"],
  "src/backend/provider-turn-executor.test.ts": ["assistantMessage"],
  "src/backend/local-provider-errors.test.ts": [],
  "bun.lock": [],
});

const CASES = Object.freeze([
  ["openai-compatible", "happy-tool", ["request_mapping", "event_order",
    "tool_call_assembly", "usage"], null, "stop", "sse"],
  ["anthropic", "reasoning-redacted", ["event_order", "usage"], null,
    "stop", "anthropic_sse"],
  ["ollama", "cancelled", ["cancellation", "errors"], "cancelled",
    "error", "ndjson"],
  ["lm-studio", "timeout", ["timeout", "errors"], "timeout", "error", "sse"],
  ["llama-cpp", "context-overflow", ["context_overflow", "errors"],
    "context_overflow", "error", "sse"],
  ["openai-compatible", "retry-after", ["retry_after", "errors"],
    "rate_limit", "error", "sse"],
  ["ollama", "image-drop", ["image_policy", "request_mapping"], null,
    "stop", "ndjson"],
  ["ollama", "image-strict", ["image_policy", "request_mapping"],
    "invalid_request", "error", "ndjson"],
  ["anthropic", "authentication", ["errors"], "authentication", "error",
    "anthropic_sse"],
  ["openai-compatible", "authorization", ["errors"], "authorization",
    "error", "sse"],
  ["lm-studio", "invalid-request", ["errors"], "invalid_request", "error", "sse"],
  ["anthropic", "quota", ["errors"], "quota", "error", "anthropic_sse"],
  ["llama-cpp", "overloaded", ["errors"], "overloaded", "error", "sse"],
  ["ollama", "unavailable", ["errors"], "unavailable", "error", "ndjson"],
  ["openai-compatible", "protocol-error", ["errors"], "protocol", "error", "sse"],
  ["lm-studio", "unknown-error", ["errors"], "unknown", "error", "sse"],
]);
const ERROR_MAP = Object.freeze([
  ["authentication", "anthropic/authentication"],
  ["authorization", "openai-compatible/authorization"],
  ["invalid_request", "ollama/image-strict"],
  ["rate_limit", "openai-compatible/retry-after"],
  ["quota", "anthropic/quota"], ["timeout", "lm-studio/timeout"],
  ["context_overflow", "llama-cpp/context-overflow"],
  ["overloaded", "llama-cpp/overloaded"], ["unavailable", "ollama/unavailable"],
  ["protocol", "openai-compatible/protocol-error"],
  ["cancelled", "ollama/cancelled"], ["unknown", "lm-studio/unknown-error"],
]);

function fail(message) {
  throw new Error(`provider fixture capture failed: ${message}`);
}
function sha(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
function json(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}
function discover(explicit) {
  if (explicit) {
    const found = resolve(explicit);
    if (!isPinned(found)) fail("explicit source is not the pinned checkout");
    return found;
  }
  const candidates = [resolve(ROOT, "../letta-code"), resolve(ROOT, "../../letta-code"),
    resolve(ROOT, "letta-code")];
  for (const candidate of candidates) if (isPinned(candidate)) return candidate;
  fail("pinned source checkout not found");
}
function isPinned(path) {
  try {
    const head = execFileSync("git", ["-C", path, "rev-parse", "HEAD"],
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
    return head === PIN;
  } catch { return false; }
}
function maskLexical(source) {
  let out = "", state = "code", quote = "", escaped = false;
  for (let i = 0; i < source.length; i += 1) {
    const c = source[i], n = source[i + 1];
    if (state === "line") {
      out += c === "\n" ? "\n" : " ";
      if (c === "\n") state = "code";
      continue;
    }
    if (state === "block") {
      out += c === "\n" ? "\n" : " ";
      if (c === "*" && n === "/") { out += " "; i += 1; state = "code"; }
      continue;
    }
    if (state === "string") {
      out += c === "\n" ? "\n" : " ";
      if (!escaped && c === quote) state = "code";
      escaped = !escaped && c === "\\";
      continue;
    }
    if (c === "/" && n === "/") { out += "  "; i += 1; state = "line"; continue; }
    if (c === "/" && n === "*") { out += "  "; i += 1; state = "block"; continue; }
    if ('"\'`'.includes(c)) { quote = c; state = "string"; escaped = false; out += " "; continue; }
    out += c;
  }
  if (state === "block" || state === "string") fail("unterminated lexical region");
  return out;
}
function operativeRegion(source, name) {
  const masked = maskLexical(source);
  const pattern = new RegExp(`(?:function|class|interface|type|const)\\s+${name}\\b`, "g");
  const hits = [...masked.matchAll(pattern)];
  if (hits.length !== 1) fail(`operative symbol ${name} is not unique`);
  const start = hits[0].index;
  const open = masked.indexOf("{", start);
  if (open < 0) fail(`operative symbol ${name} has no body`);
  let depth = 0;
  for (let i = open; i < masked.length; i += 1) {
    if (masked[i] === "{") depth += 1;
    if (masked[i] === "}") depth -= 1;
    if (depth === 0) return source.slice(start, i + 1);
  }
  fail(`operative symbol ${name} is unbalanced`);
}
function verifySources(sourceRoot) {
  const regions = [];
  for (const [path, expected] of Object.entries(SOURCE_HASHES)) {
    const bytes = readFileSync(resolve(sourceRoot, path));
    if (sha(bytes) !== expected) fail(`whole-file pin mismatch: ${path}`);
    const text = bytes.toString("utf8");
    for (const name of REGION_NAMES[path]) {
      const region = operativeRegion(text, name);
      regions.push({ path, symbol: name, sha256: sha(region) });
    }
  }
  const lock = readFileSync(resolve(sourceRoot, "bun.lock"), "utf8");
  const prefix = '"@earendil-works/pi-ai": [';
  const lines = lock.split("\n").filter((line) => line.trim().startsWith(prefix));
  if (lines.length !== 1) fail("resolved pi-ai lock entry count");
  const parsed = JSON.parse(`{${lines[0].trim().replace(/,$/, "")}}`);
  const resolved = parsed["@earendil-works/pi-ai"];
  if (!Array.isArray(resolved) || resolved[0] !== `@earendil-works/pi-ai@${PI_AI_VERSION}`) {
    fail("resolved pi-ai lock version mismatch");
  }
  return regions;
}
function requestFor(dialect, name) {
  const image = name.startsWith("image-");
  return {
    model: { handle: `${dialect}/fixture-model`, provider_id: dialect,
      available: true, context_window: 8192, model_settings: { fixture: true } },
    system: "SANITIZED_FIXTURE_SYSTEM", messages: [{ role: "user",
      content: [{ type: "text", text: "SANITIZED_FIXTURE_USER" },
        ...(image ? [{ type: "image", media_type: "image/png", base64: "UE5H" }] : [])],
      tool_call_id: null }], tools: [{ name: "fixture_tool",
      description: "SANITIZED_FIXTURE_TOOL", input_schema: { type: "object",
        properties: { a: { type: "integer" } }, required: ["a"] } }],
    tool_choice: "auto", image_policy: name === "image-strict" ? "strict" : "drop",
    context_tokens_max: 8192, output_tokens_max: 256,
    reasoning: { enabled: name === "reasoning-redacted", effort: "medium", tier: null },
    deadline_ms: 1000,
  };
}
function expectedRequest(dialect, name) {
  if (name === "image-strict") return { outcome: "preflight_error",
    error_kind: "invalid_request", reason: "SANITIZED_FIXTURE_UNSUPPORTED_IMAGE_STRICT" };
  const message = { role: "user", content: "SANITIZED_FIXTURE_USER" };
  const tool = { type: "function", function: { name: "fixture_tool",
    description: "SANITIZED_FIXTURE_TOOL", parameters: { type: "object",
      properties: { a: { type: "integer" } }, required: ["a"] } } };
  if (dialect === "anthropic") return { method: "POST", endpoint: "/v1/messages",
    headers: { "anthropic-version": "SANITIZED_FIXTURE_VERSION" }, body: {
      model: "fixture-model", system: "SANITIZED_FIXTURE_SYSTEM", messages: [message],
      max_tokens: 256, stream: true, tools: [{ name: "fixture_tool",
        description: "SANITIZED_FIXTURE_TOOL", input_schema: tool.function.parameters }],
      thinking: name === "reasoning-redacted" ? { type: "enabled", budget_tokens: 128 } :
        { type: "disabled" } } };
  if (dialect === "ollama") return { method: "POST", endpoint: "/api/chat", body: {
    model: "fixture-model", messages: [{ role: "system", content: "SANITIZED_FIXTURE_SYSTEM" },
      { ...message, images: name === "image-drop" ? [] : undefined }], stream: true,
    tools: [tool], options: { num_ctx: 8192, num_predict: 256 } } };
  const marker = dialect === "lm-studio" ? "lm-studio" :
    dialect === "llama-cpp" ? "llama.cpp" : "openai-compatible";
  return { method: "POST", endpoint: "/v1/chat/completions", compatibility: marker, body: {
    model: "fixture-model", messages: [{ role: "system", content: "SANITIZED_FIXTURE_SYSTEM" },
      message], tools: [tool], tool_choice: "auto", max_tokens: 256, stream: true,
    stream_options: { include_usage: true } } };
}
function provenance(symbol, boundary = "pinned_pi_ai_test_boundary") {
  return { source_test: "src/backend/pi-stream-adapter.test.ts", source_symbol: symbol,
    capture_boundary: boundary };
}
function rawEvent(type, fields = {}, symbol = "streamFromEvents", boundary) {
  return { type, ...fields, provenance: provenance(symbol, boundary) };
}
function rawError(kind, name) {
  return rawEvent("error", { kind, code: `fixture_${kind}`,
    context: `SANITIZED_FIXTURE_${name.toUpperCase().replaceAll("-", "_")}` },
  "normalizeLocalProviderError", "pinned_error_normalization_boundary");
}
function baselineFor(dialect, name, errorKind) {
  if (name === "happy-tool") return [rawEvent("text_delta", { text: "SANITIZED_FIXTURE_TEXT" }),
    rawEvent("metadata", {
      entries: [{ key: "fixture_request_id", value: "SANITIZED_FIXTURE_ID" }],
    }),
    rawEvent("toolcall_start", { call_id: "fixture-call", name: "fixture_tool" }),
    rawEvent("toolcall_arguments_delta", { call_id: "fixture-call", arguments: "{\"a\":" }),
    rawEvent("toolcall_arguments_delta", { call_id: "fixture-call", arguments: "1}" }),
    rawEvent("toolcall_end", { call_id: "fixture-call" }), usage(5, 2), usage(5, 4),
    rawEvent("done", { reason: "tool_use" })];
  if (name === "reasoning-redacted") return [
    rawEvent("thinking_delta", { text: "SANITIZED_FIXTURE_REASONING" }),
    rawEvent("redacted_reasoning", { marker: "<redacted-fixture>" }, "assistantMessage",
      "pinned_reasoning_replay_boundary"),
    rawEvent("text_delta", { text: "SANITIZED_FIXTURE_TEXT" }),
    usage(4, 2), rawEvent("done", { reason: "end_turn" })];
  if (name === "cancelled") return [rawEvent("text_delta", { text: "SANITIZED_FIXTURE_TEXT" }),
    rawEvent("cancelled", { code: "fixture_cancelled", context: "SANITIZED_FIXTURE_CANCELLED" }),
    rawEvent("late_text_delta", { text: "SANITIZED_FIXTURE_LATE" })];
  if (!errorKind) return [rawEvent("text_delta", { text: "SANITIZED_FIXTURE_TEXT" }), usage(3, 2),
    rawEvent("done", { reason: "end_turn" })];
  return [rawError(errorKind, name)];
}
function usage(input, output) {
  return rawEvent("usage", { input_tokens: input, output_tokens: output,
    cached_input_tokens: 0, reasoning_tokens: 0 }, "createUsageStatisticsChunk");
}
function normalizeBaseline(events) {
  const output = [], tools = new Map(); let terminal = false;
  for (const source of events) {
    if (terminal || source.type === "late_text_delta") continue;
    const type = source.type;
    if (type === "text_delta") output.push({ type: "TextDelta", text: source.text });
    else if (type === "thinking_delta") output.push({ type: "ReasoningDelta", text: source.text });
    else if (type === "redacted_reasoning") output.push({
      type: "RedactedReasoning",
      marker: source.marker,
    });
    else if (type === "metadata") output.push({
      type: "ProviderMetadata",
      entries: source.entries,
    });
    else if (type === "toolcall_start") { tools.set(source.call_id, ""); output.push({
      type: "ToolCallStart", call_id: source.call_id, name: source.name }); }
    else if (type === "toolcall_arguments_delta") { tools.set(source.call_id,
      `${tools.get(source.call_id) ?? ""}${source.arguments}`); output.push({
      type: "ToolCallArgumentsDelta", call_id: source.call_id,
      bytes_base64: Buffer.from(source.arguments).toString("base64") }); }
    else if (type === "toolcall_end") { JSON.parse(tools.get(source.call_id)); output.push({
      type: "ToolCallEnd", call_id: source.call_id }); }
    else if (type === "usage") output.push({ type: "Usage", input_tokens: source.input_tokens,
      output_tokens: source.output_tokens, cached_input_tokens: source.cached_input_tokens,
      reasoning_tokens: source.reasoning_tokens });
    else if (type === "done") {
      output.push({ type: "Stop", reason: source.reason });
      terminal = true;
    }
    else if (type === "cancelled") { output.push({ type: "Error", kind: "cancelled",
      code: source.code, context: source.context }); terminal = true; }
    else if (type === "error") { output.push({ type: "Error", kind: source.kind,
      code: source.code, context: source.context }); terminal = true; }
    else fail(`unknown baseline event: ${type}`);
  }
  return output;
}
function openAIRaw(dialect, name, errorKind) {
  if (errorKind) return `status: ${errorKind === "rate_limit" ? 429 : 500}\n` +
    `${errorKind === "rate_limit" ? "header: retry-after=2\n" : ""}` +
    `data: ${JSON.stringify({ error: { type: errorKind, code: `fixture_${errorKind}`,
      message: `SANITIZED_FIXTURE_${name}` } })}\n`;
  const model = dialect === "lm-studio" ? "lmstudio-fixture-model" :
    dialect === "llama-cpp" ? "llama-fixture-model" : "fixture-model";
  const first = { id: "fixture-chat", object: "chat.completion.chunk", model,
    system_fingerprint: dialect === "lm-studio" ? "lmstudio-synthetic" :
      dialect === "llama-cpp" ? "llama.cpp-synthetic" : "openai-synthetic",
    choices: [{ index: 0, delta: { content: "SANITIZED_FIXTURE_TEXT",
      tool_calls: [{ index: 0, id: "fixture-call", type: "function",
        function: { name: "fixture_tool", arguments: "{\"a\":" } }] }, finish_reason: null }] };
  const second = { id: "fixture-chat", object: "chat.completion.chunk", model,
    choices: [{ index: 0, delta: { tool_calls: [{ index: 0,
      function: { arguments: "1}" } }] }, finish_reason: "tool_calls" }],
    usage: { prompt_tokens: 5, completion_tokens: 4, total_tokens: 9 },
    ...(dialect === "llama-cpp" ? { timings: { prompt_n: 5, predicted_n: 4 } } : {}) };
  return `data: ${JSON.stringify(first)}\n\ndata: ${JSON.stringify(second)}\n\n` +
    "data: [DONE]\n";
}
function anthropicRaw(name, errorKind) {
  if (errorKind) return `event: error\ndata: ${JSON.stringify({ type: "error", error: {
    type: errorKind, message: `SANITIZED_FIXTURE_${name}` } })}\n`;
  const events = [
    ["message_start", {
      type: "message_start",
      message: {
        id: "fixture-message",
        type: "message",
        role: "assistant",
        model: "fixture-model",
        content: [],
        usage: { input_tokens: 4, output_tokens: 0 },
      },
    }],
    ["content_block_start", { type: "content_block_start", index: 0,
      content_block: { type: "thinking", thinking: "" } }],
    ["content_block_delta", { type: "content_block_delta", index: 0,
      delta: { type: "thinking_delta", thinking: "SANITIZED_FIXTURE_REASONING" } }],
    ["content_block_start", { type: "content_block_start", index: 1,
      content_block: { type: "redacted_thinking", data: "SANITIZED_FIXTURE_REDACTED" } }],
    ["content_block_start", { type: "content_block_start", index: 2,
      content_block: { type: "text", text: "" } }],
    ["content_block_delta", { type: "content_block_delta", index: 2,
      delta: { type: "text_delta", text: "SANITIZED_FIXTURE_TEXT" } }],
    ["message_delta", { type: "message_delta", delta: { stop_reason: "end_turn" },
      usage: { output_tokens: 2 } }], ["message_stop", { type: "message_stop" }]];
  return `${events.map(([event, data]) =>
    `event: ${event}\ndata: ${JSON.stringify(data)}`).join("\n\n")}\n`;
}
function ollamaRaw(name, errorKind) {
  if (errorKind) return `${JSON.stringify({ error: { type: errorKind,
    message: `SANITIZED_FIXTURE_${name}` } })}\n`;
  return `${JSON.stringify({ model: "fixture-model", created_at: "2000-01-01T00:00:00Z",
    message: { role: "assistant", content: "SANITIZED_FIXTURE_TEXT", tool_calls: [{ function: {
      name: "fixture_tool", arguments: { a: 1 } } }] }, done: false })}\n` +
    `${JSON.stringify({ model: "fixture-model", message: { role: "assistant", content: "" },
      done: true, done_reason: "stop", prompt_eval_count: 3, eval_count: 2 })}\n`;
}
function rawFor(dialect, name, errorKind) {
  if (dialect === "anthropic") return anthropicRaw(name, errorKind);
  if (dialect === "ollama") return ollamaRaw(name, errorKind);
  return openAIRaw(dialect, name, errorKind);
}
function buildCorpus(regions) {
  const files = new Map();
  const records = [];
  for (const row of CASES) {
    const [dialect, name, dimensions, errorKind, terminalKind, rawFormat] = row;
    const root = `${dialect}/${name}`;
    const baseline = baselineFor(dialect, name, errorKind);
    const paths = ["request.json", "expected-request.json", "raw-stream.txt",
      "baseline-events.jsonl", "expected-trace.json"].map((file) => `${root}/${file}`);
    put(files, paths[0], json(requestFor(dialect, name)));
    put(files, paths[1], json(expectedRequest(dialect, name)));
    put(files, paths[2], rawFor(dialect, name, errorKind));
    put(files, paths[3], `${baseline.map(JSON.stringify).join("\n")}\n`);
    put(files, paths[4], json(normalizeBaseline(baseline)));
    records.push({ id: root, dialect, name, paths, raw_format: rawFormat,
      dimensions, reasoning: name === "reasoning-redacted" ?
        { visible: true, redacted: true } : { visible: false, redacted: false },
      terminal_kind: terminalKind });
  }
  const inventory = [...files].map(([path, bytes]) => ({ path,
    kind: path.endsWith(".jsonl") ? "jsonl" : path.endsWith(".json") ? "json" : "raw",
    bytes: bytes.length, sha256: sha(bytes) })).sort((a, b) => a.path.localeCompare(b.path));
  const index = { schema_version: 1, source_commit: PIN, pi_ai_version: PI_AI_VERSION,
    generator: "tools/capture-provider-streams.mjs",
    provenance: "Deterministic capture from pinned baseline mock/pi-ai boundary and source tests",
    capture_boundary: "Sanitized source-test fixtures, not live API traffic",
    source_regions: regions, dialects: DIALECTS, dimensions: DIMENSIONS, cases: records,
    error_kind_to_case: ERROR_MAP, inventory };
  return { files, index };
}
function put(files, path, text) {
  const bytes = Buffer.from(text);
  const total = [...files.values()].reduce((sum, item) => sum + item.length, 0);
  const names = path.split("/");
  if (files.size >= LIMITS.files || bytes.length > LIMITS.bytes ||
      total + bytes.length > LIMITS.totalBytes || files.has(path) ||
      names.length > LIMITS.depth || names.some((name) => !name || name.length > LIMITS.name)) {
    fail("corpus bound");
  }
  sanitize(path, bytes);
  files.set(path, bytes);
}
function sanitize(path, bytes) {
  const text = bytes.toString("utf8");
  if (!Buffer.from(text).equals(bytes)) fail(`non-UTF8 fixture: ${path}`);
  const decoded = text.replace(/\u([0-9a-f]{4})/gi, (_, value) =>
    String.fromCharCode(Number.parseInt(value, 16))).replace(/\r|\n|\t/g, " ");
  const forbidden = [/Bearer\s/i, /-----\s*BEGIN\s+[A-Z ]*PRIVATE\s+KEY\s*-----/i,
    /eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/, /api[_ -]?key/i,
    /password/i, /access[_ -]?token/i, /refresh[_ -]?token/i, /client[_ -]?secret/i,
    /oauth/i, /sk-[A-Za-z0-9]{12,}/];
  if (forbidden.some((rule) => rule.test(decoded))) fail(`unsanitized fixture: ${path}`);
  if (!text.includes("SANITIZED_FIXTURE") && !path.endsWith("expected-trace.json")) {
    fail(`fixture lacks sanitized marker: ${path}`);
  }
}
function writeCorpus(corpus) {
  const parent = dirname(OUT);
  const temp = mkdtempSync(resolve(parent, ".providers-"));
  try {
    for (const [path, bytes] of corpus.files) {
      const target = resolve(temp, path);
      confine(temp, target);
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, bytes, { mode: 0o644 });
    }
    writeFileSync(resolve(temp, "index.json"), json(corpus.index), { mode: 0o644 });
    validateTree(temp, corpus);
    const old = resolve(parent, `.providers-backup-${process.pid}`);
    rmSync(old, { recursive: true, force: true });
    let moved = false;
    try { renameSync(OUT, old); moved = true; }
    catch (error) { if (error.code !== "ENOENT") throw error; }
    try { renameSync(temp, OUT); }
    catch (error) { if (moved) renameSync(old, OUT); throw error; }
    rmSync(old, { recursive: true, force: true });
  } finally { rmSync(temp, { recursive: true, force: true }); }
}
function confine(root, target) {
  if (target !== root && !target.startsWith(`${root}${sep}`)) fail("path escape");
}
function scanTree(root) {
  const output = new Map(), stack = [[root, 0]];
  while (stack.length) {
    const [dir, depth] = stack.pop(); if (depth > LIMITS.depth) fail("tree depth");
    const names = readdirSync(dir);
    if (depth > 0 && names.length === 0) fail("empty directory");
    for (const name of names) {
      if (!name || name.length > LIMITS.name) fail("tree name bound");
      const path = resolve(dir, name), stat = lstatSync(path);
      const rel = relative(root, path).replaceAll(sep, "/");
      if (stat.isSymbolicLink() || (!stat.isFile() && !stat.isDirectory())) {
        fail("special tree entry");
      }
      if (stat.isDirectory()) stack.push([path, depth + 1]);
      else {
        if (output.size >= LIMITS.files + 1) fail("tree file bound");
        output.set(rel, readFileSync(path));
      }
    }
  }
  return output;
}
function validateTree(root, corpus) {
  const actual = scanTree(root), expected = new Map(corpus.files);
  expected.set("index.json", Buffer.from(json(corpus.index)));
  if (actual.size !== expected.size) fail("tree file count mismatch");
  for (const [path, bytes] of expected) {
    const found = actual.get(path);
    if (!found || !found.equals(bytes)) fail(`tree mismatch: ${path}`);
  }
}
function checkCorpus(corpus) { validateTree(OUT, corpus); }
function selfTest() {
  const sample = "// decoy foo { }\nfunction foo(){return {a:'}'};}\n/* foo(){} */";
  if (!operativeRegion(sample, "foo").includes("return")) fail("operative self-test");
  const badRegions = [
    "function foo(){} function foo(){}", "function foo(){", "function foo(){'x}",
  ];
  for (const bad of badRegions) {
    let rejected = false; try { operativeRegion(bad, "foo"); } catch { rejected = true; }
    if (!rejected) fail("operative guard self-test");
  }
  const late = normalizeBaseline([rawEvent("done", { reason: "end_turn" }),
    rawEvent("late_text_delta", { text: "SANITIZED_FIXTURE_LATE" })]);
  if (late.length !== 1) fail("terminal suppression self-test");
  const unsafe = Buffer.from("Bearer SANITIZED_FIXTURE");
  let rejected = false; try { sanitize("fixture", unsafe); } catch { rejected = true; }
  if (!rejected) fail("sanitization self-test");
  const corpus = buildCorpus([{ path: "fixture", symbol: "fixture", sha256: sha("fixture") }]);
  if (corpus.files.size !== 80) fail("corpus bound self-test");
}
function parseArgs(argv) {
  let explicit, check = false, self = false;
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--source") {
      if (i + 1 >= argv.length || argv[i + 1].startsWith("--")) fail("--source requires path");
      explicit = argv[++i];
    }
    else if (argv[i] === "--check") check = true;
    else if (argv[i] === "--self-test") self = true;
    else fail(`unknown argument: ${argv[i]}`);
  }
  return { explicit, check, self };
}
function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.self) { selfTest(); return; }
  const source = discover(args.explicit);
  if (!isPinned(source)) fail("source HEAD changed before reads");
  const corpus = buildCorpus(verifySources(source));
  if (args.check) checkCorpus(corpus); else writeCorpus(corpus);
}
try { main(); } catch (error) {
  const message = error instanceof Error ? error.message : "unknown failure";
  process.stderr.write(`${message.slice(0, 240)}\n`); process.exitCode = 1;
}
