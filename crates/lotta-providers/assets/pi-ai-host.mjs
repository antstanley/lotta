import { readdir, realpath, readFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import { createServer } from "node:http";

const PIN = "0.82.1";
const PROTOCOL = 1;
const FRAME_MAX = 36 * 1024 * 1024;
const TIMEOUT_MAX_MS = 300_000;
const REQUESTS_MAX = 64;
const PROVIDERS_MAX = 128;
const MODELS_MAX = 10_000;
const TEXT_MAX = 512;
const STREAMS_MAX = 64;
const EVENTS_MAX = 4096;
const STREAM_QUEUE_MAX = 64;
const EVENT_BYTES_MAX = 4 * 1024 * 1024;
const CREDENTIAL_BYTES_MAX = 64 * 1024;
const FIXTURE_BYTES_MAX = 8 * 1024 * 1024;
const OAUTH_TEXT_MAX = 8 * 1024;
const OPENAI_OAUTH = Object.freeze({
  provider: "openai-codex",
  issuer: "https://auth.openai.com",
  authorization_endpoint: "https://auth.openai.com/oauth/authorize",
  token_endpoint: "https://auth.openai.com/oauth/token",
  device_authorization_endpoint: "https://auth.openai.com/api/accounts/deviceauth/usercode",
  device_token_endpoint: "https://auth.openai.com/api/accounts/deviceauth/token",
  device_verification_uri: "https://auth.openai.com/codex/device",
  client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
  audience: null,
  scopes: ["openid", "profile", "email", "offline_access"],
  redirect_uris: ["http://localhost:1455/auth/callback", "https://auth.openai.com/deviceauth/callback"]
});
const TEST_MODE = process.env.LOTTA_HOST_TEST_MODE === "1";
const packageRoot = process.argv[2];
if (!packageRoot) process.exit(64);
let owner;
let nonce;
let manifestVersion;

let stopped = false;
let input = Buffer.alloc(0);
let processing = false;
let activeRequests = 0;
let writer = Promise.resolve();
const registrations = new Map();
const streams = new Map();
let streamCounter = 0;
let revision = 0;
let pi;
let openAICompletionsApi;
let anthropicMessagesApi;

function boundedText(value, name) {
  if (typeof value !== "string" || value.length === 0 || Buffer.byteLength(value) > TEXT_MAX) {
    throw new Error(`invalid ${name}`);
  }
  return value;
}

function oauthText(value, name) {
  if (typeof value !== "string" || value.length === 0 || Buffer.byteLength(value) > OAUTH_TEXT_MAX || /[\r\n\0]/.test(value))
    throw new Error(`invalid oauth ${name}`);
  return value;
}

function oauthDispatch(command, params) {
  if (command === "oauth.metadata") return structuredClone(OPENAI_OAUTH);
  if (params?.provider !== OPENAI_OAUTH.provider || typeof params?.session !== "string")
    throw new Error("oauth binding rejected");
  if (command === "oauth.begin") {
    oauthText(params.flow_id, "flow"); oauthText(params.authorization_url, "url");
    const url = new URL(params.authorization_url);
    if (url.origin !== OPENAI_OAUTH.issuer || url.pathname !== "/oauth/authorize" ||
        url.searchParams.get("client_id") !== OPENAI_OAUTH.client_id ||
        url.searchParams.get("redirect_uri") !== OPENAI_OAUTH.redirect_uris[0] ||
        !Number.isSafeInteger(params.expires_at)) throw new Error("oauth begin rejected");
    return { flow_id: params.flow_id, authorization_url: params.authorization_url, expires_at: params.expires_at };
  }
  if (command === "oauth.exchange") {
    oauthText(params.flow_id, "flow"); oauthText(params.code, "code"); oauthText(params.state, "state");
    if (params.origin !== "http://localhost:1455" || params.redirect_uri !== OPENAI_OAUTH.redirect_uris[0])
      throw new Error("oauth callback rejected");
    return { accepted: true };
  }
  if (command === "oauth.device.begin") {
    oauthText(params.flow_id, "flow"); oauthText(params.user_code, "user code");
    if (params.verification_uri !== OPENAI_OAUTH.device_verification_uri ||
        !Number.isSafeInteger(params.interval_seconds) || params.interval_seconds < 1 ||
        !Number.isSafeInteger(params.expires_at)) throw new Error("oauth device rejected");
    return { flow_id: params.flow_id, user_code: params.user_code,
      verification_uri: params.verification_uri, interval_seconds: params.interval_seconds,
      expires_at: params.expires_at };
  }
  if (command === "oauth.device.poll") {
    oauthText(params.flow_id, "flow"); return { accepted: true };
  }
  if (command === "oauth.cancel") {
    oauthText(params.flow_id, "flow"); return { cancelled: true };
  }
  throw new Error("unknown oauth command");
}

function send(kind, requestId, payload) {
  const value = { version: PROTOCOL, owner, capability: "provider", timeout_ms: TIMEOUT_MAX_MS,
    request_id: requestId, correlation_id: null, kind, payload };
  const body = Buffer.from(JSON.stringify(value));
  if (body.length === 0 || body.length > FRAME_MAX) return Promise.reject(new Error("output frame bound"));
  const prefix = Buffer.allocUnsafe(4);
  prefix.writeUInt32BE(body.length);
  writer = writer.then(async () => {
    if (!process.stdout.write(prefix)) await new Promise((resolve) => process.stdout.once("drain", resolve));
    if (!process.stdout.write(body)) await new Promise((resolve) => process.stdout.once("drain", resolve));
  });
  return writer;
}

async function loadPinned() {
  const root = await realpath(packageRoot);
  if (root !== packageRoot || !(await stat(root)).isDirectory()) throw new Error("package root");
  const manifestPath = join(root, "package.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  if (manifest.name !== "@earendil-works/pi-ai" || manifest.version !== PIN) {
    throw new Error("pi-ai version mismatch");
  }
  const entry = await realpath(join(root, "dist/providers/all.js"));
  if (dirname(dirname(dirname(entry))) !== root) throw new Error("module confinement");
  manifestVersion = manifest.version;
  pi = await import(pathToFileURL(entry).href);
  ({ openAICompletionsApi } = await import(pathToFileURL(join(root, "dist/api/openai-completions.lazy.js")).href));
  ({ anthropicMessagesApi } = await import(pathToFileURL(join(root, "dist/api/anthropic-messages.lazy.js")).href));
}

const bytewise = (a, b) => Buffer.compare(Buffer.from(String(a)), Buffer.from(String(b)));
const cost = (value) => value === -1_000_000 ? null : value;
function modelDescriptor(model) {
  if (!model || !Array.isArray(model.input) || !model.cost) throw new Error("invalid model");
  return {
    id: boundedText(model.id, "model id"), name: boundedText(model.name, "model name"),
    api: boundedText(model.api, "api"), provider: boundedText(model.provider, "provider"),
    reasoning: model.reasoning === true, input: [...model.input].sort(bytewise),
    context_window: model.contextWindow, max_tokens: model.maxTokens,
    cost: { input: cost(model.cost.input), output: cost(model.cost.output),
      cache_read: cost(model.cost.cacheRead), cache_write: cost(model.cost.cacheWrite),
      tiers: (model.cost.tiers ?? []).map((tier) => ({ input_tokens_above: tier.inputTokensAbove,
        input: cost(tier.input), output: cost(tier.output), cache_read: cost(tier.cacheRead),
        cache_write: cost(tier.cacheWrite) })) }
  };
}

function builtinCatalog() {
  const providers = pi.builtinProviders();
  if (!Array.isArray(providers) || providers.length > PROVIDERS_MAX) throw new Error("provider bound");
  const result = [];
  let count = 0;
  for (const provider of providers) {
    const models = pi.getBuiltinModels(provider.id) ?? [];
    count += models.length;
    if (count > MODELS_MAX) throw new Error("model bound");
    result.push({ id: boundedText(provider.id, "provider id"),
      name: boundedText(provider.name, "provider name"), source: "builtin",
      models: models.map(modelDescriptor) });
  }
  for (const registration of registrations.values()) result.push(registration);
  result.sort((a, b) => bytewise(a.id, b.id));
  for (const provider of result) provider.models.sort((a, b) => bytewise(a.id, b.id));
  return result;
}

function validateRegistration(value) {
  const id = boundedText(value?.id, "provider id");
  const name = boundedText(value?.name, "provider name");
  const registrationOwner = boundedText(value?.owner, "registration owner");
  if (!value.adapter || value.adapter.type !== "pi_ai_api" ||
      !["openai-completions", "anthropic-messages"].includes(value.adapter.api))
    throw new Error("invalid provider adapter");
  if (!Array.isArray(value.models) || value.models.length > MODELS_MAX) throw new Error("models");
  const models = value.models.map(modelDescriptor).sort((a, b) => bytewise(a.id, b.id));
  if (models.some((model) => model.provider !== id || model.api !== value.adapter.api))
    throw new Error("provider mismatch");
  if (models.some((model, index) => index && models[index - 1].id === model.id)) throw new Error("duplicate model");
  return { id, name, owner: registrationOwner, source: "mod", adapter: { ...value.adapter }, models,
    getModels() { return this.models; },
    streamSimple(model, context, options) {
      const api = model.api === "openai-completions" ? openAICompletionsApi() : anthropicMessagesApi();
      return api.streamSimple(model, context, options);
    } };
}

function retryAfterMs(error, transport) {
  const headers = error?.headers ?? error?.response?.headers ?? transport?.headers;
  const rawMilliseconds = headers?.["retry-after-ms"] ?? headers?.get?.("retry-after-ms");
  const milliseconds = Number(rawMilliseconds);
  if (rawMilliseconds != null && Number.isFinite(milliseconds) && milliseconds >= 0)
    return Math.round(milliseconds);
  const rawSeconds = headers?.["retry-after"] ?? headers?.get?.("retry-after");
  const seconds = Number(rawSeconds);
  return rawSeconds != null && Number.isFinite(seconds) && seconds >= 0
    ? Math.round(seconds * 1000) : undefined;
}

function errorDetails(error, transport) {
  const message = String(error?.errorMessage ?? error?.message ?? "");
  const statusPrefix = message.match(/^\s*(\d{3})\b/);
  let nested;
  const jsonStart = message.indexOf("{");
  if (jsonStart >= 0) {
    try { nested = JSON.parse(message.slice(jsonStart)); } catch {}
  }
  const body = nested?.error ?? nested;
  const status = Number(error?.status ?? error?.statusCode ?? error?.response?.status ??
    transport?.status ?? statusPrefix?.[1]);
  const probe = [error?.type, error?.code, error?.name, message, body?.type, body?.code,
    body?.message].filter(Boolean).join(" ").toLowerCase();
  return { status, probe, body };
}

function errorKind(error, transport) {
  const { status, probe } = errorDetails(error, transport);
  if (probe.includes("cancel") || probe.includes("abort")) return "cancelled";
  if (status === 401 || /authenticat|invalid.api.key|unauthorized/.test(probe)) return "authentication";
  if (status === 403 || /permission|forbidden|authoriz/.test(probe)) return "authorization";
  if ((status === 429 || status === 402) && /quota|credit|billing/.test(probe)) return "quota";
  if (status === 429 || /rate.?limit|too.many.requests/.test(probe)) return "rate_limit";
  if (status === 408 || status === 504 || /timeout|timed.out|deadline/.test(probe)) return "timeout";
  if (/context.{0,12}(length|window|overflow)|too.many.tokens|maximum.context/.test(probe)) return "context_overflow";
  if (status === 400 || status === 404 || status === 422 || /invalid.request|bad.request|unknown.model/.test(probe)) return "invalid_request";
  if (status === 529 || /overload|capacity|busy/.test(probe)) return "overloaded";
  if ([502, 503].includes(status) || /unavailable|connection|network|econn/.test(probe)) return "unavailable";
  if (/protocol|parse|malformed|invalid.response|schema/.test(probe)) return "protocol";
  return "unknown";
}

function safeError(error, transport) {
  if (error?.fixtureFailure) return { type: "FixtureAssertionFailure", context: error.message };
  const details = errorDetails(error, transport);
  const kind = errorKind(error, transport);
  const rawCode = String(details.body?.code ?? details.body?.type ?? error?.code ?? error?.type ??
    `${kind}_error`).toLowerCase();
  const code = rawCode.replace(/[^a-z0-9_.-]/g, "_").slice(0, TEXT_MAX) || "provider_error";
  const rawContext = String(details.body?.message ?? error?.errorMessage ?? error?.message ??
    `provider request failed (${kind})`);
  const context = rawContext.replace(/[\r\n\0]/g, " ").slice(0, TEXT_MAX);
  const retry = retryAfterMs(error, transport);
  return { type: "Error", kind, code, context, ...(retry === undefined ? {} : { retry_after_ms: retry }) };
}

function validateAuth(auth) {
  const bytes = Buffer.byteLength(JSON.stringify(auth ?? null));
  if (!auth || bytes > CREDENTIAL_BYTES_MAX) throw new Error("credential bound");
  if (!["none", "api_key", "oauth_access", "aws", "google_credentials"].includes(auth.type))
    throw new Error("auth type");
}

function piContext(request) {
  const content = (parts, role) => {
    const mapped = parts.map((part) => part.type === "image"
      ? { type: "image", data: part.base64, mimeType: part.media_type }
      : { type: "text", text: part.text });
    return role === "user" && mapped.length === 1 && mapped[0].type === "text" ? mapped[0].text : mapped;
  };
  return {
    systemPrompt: request.system ?? undefined,
    messages: request.messages.map((message, index) => message.role === "tool"
      ? { role: "toolResult", toolCallId: message.tool_call_id, toolName: message.tool_name ?? "tool",
          content: content(message.content, message.role), isError: message.is_error === true, timestamp: index }
      : message.role === "assistant" ? { role: "assistant", content: content(message.content, message.role),
          ...(message.tool_calls ? { toolCalls: message.tool_calls } : {}), timestamp: index }
      : { role: message.role, content: content(message.content, message.role), timestamp: index }),
    tools: request.tools.map((tool) => ({ name: tool.name, description: tool.description,
      parameters: tool.input_schema }))
  };
}

function explicitOptions(params, controller) {
  const auth = params.auth;
  const provider = params.options?.provider ?? {};
  const timeoutMs = Math.max(1, Math.min(TIMEOUT_MAX_MS, params.request.deadline_ms));
  const options = { ...provider, signal: controller.signal,
    maxTokens: Number(params.request.output_tokens_max), maxRetries: 0, timeoutMs,

    env: params.options?.env ?? {}, headers: params.options?.headers ?? {},
    ...(params.request.tool_choice ? { toolChoice: params.request.tool_choice.type === "named"
      ? { type: "tool", name: params.request.tool_choice.name } : params.request.tool_choice.type } : {}) };
  if (auth.type === "api_key" || auth.type === "oauth_access") options.apiKey = auth.value;
  if (auth.type === "aws") options.env = { ...options.env, AWS_ACCESS_KEY_ID: auth.access_key_id,
    AWS_SECRET_ACCESS_KEY: auth.secret_access_key, AWS_REGION: auth.region,
    ...(auth.session_token ? { AWS_SESSION_TOKEN: auth.session_token } : {}) };
  if (auth.type === "google_credentials") throw new Error("google credentials unsupported");
  if (params.request.reasoning?.enabled) options.reasoning = params.request.reasoning.effort ?? "medium";
  return options;
}

function usageFromMessage(message) {
  const usage = message?.usage;
  if (!usage || !(usage.output > 0)) return null;
  return { type: "Usage", input_tokens: usage.input + usage.cacheRead + usage.cacheWrite,
    output_tokens: usage.output, cached_input_tokens: usage.cacheRead,
    reasoning_tokens: usage.reasoning ?? 0 };
}

function eventFromPi(event, state) {
  const toolState = state.tools;
  const snapshot = usageFromMessage(event.partial);
  if (snapshot) state.usages.set(JSON.stringify(snapshot), snapshot);
  if (event.type === "text_delta") return { type: "TextDelta", text: event.delta };
  if (event.type === "thinking_delta") return { type: "ReasoningDelta", text: event.delta };
  if (event.type === "thinking_start" || event.type === "thinking_end") {
    const block = event.partial?.content?.[event.contentIndex];
    if (block?.redacted) return { type: "RedactedReasoning", marker: "<redacted-reasoning>" };
  }
  if (event.type === "toolcall_start") {
    const call = event.partial?.content?.[event.contentIndex];
    const id = call?.id || `tool-${event.contentIndex}`;
    const name = call?.name || "unknown_tool";
    toolState.set(event.contentIndex, id);
    return { type: "ToolCallStart", call_id: id, name };
  }
  if (event.type === "toolcall_delta") {
    const id = toolState.get(event.contentIndex);
    if (!id) throw new Error("tool delta");
    return { type: "ToolCallArgumentsDelta", call_id: id,
      bytes_base64: Buffer.from(event.delta).toString("base64") };
  }
  if (event.type === "toolcall_end") return { type: "ToolCallEnd", call_id: event.toolCall.id };
  if (event.type === "done") {
    const finalUsage = usageFromMessage(event.message);
    if (finalUsage) state.usages.set(JSON.stringify(finalUsage), finalUsage);
    const entries = [];
    if (event.message.responseId) entries.push({ key: "id", value: event.message.responseId });
    if (event.message.model) entries.push({ key: "model", value: event.message.model });
    const metadata = entries.length ? [{ type: "ProviderMetadata", entries }] : [];
    const usages = [...state.usages.values()];
    const completed = event.message.api === "anthropic-messages"
      ? [...usages, ...metadata] : [...metadata, ...usages];
    return [...completed,
      { type: "Stop", reason: event.reason === "length" ? "output_limit"
        : event.reason === "toolUse" ? "tool_use" : "end_turn" }];
  }
  if (event.type === "error") return safeError(event.error ?? event, state.transport);
  return null;
}

async function collectPi(params, stream) {
  try {
    validateAuth(params.auth);
    const request = params.request;
    const provider = registrations.get(request.model.provider) ??
      pi.builtinProviders().find((entry) => entry.id === request.model.provider);
    if (!provider) throw new Error("unknown provider");
    let model = provider.getModels().find((entry) => entry.id === request.model.id);
    if (!model) throw new Error("unknown model");
    let fixture;
    if (params.fixture) fixture = await fixtureLoopback(params.fixture);
    if (fixture) model = { ...model, baseUrl: fixture.baseUrl,
      maxTokens: request.output_tokens_max,
      compat: { ...(model.compat ?? {}), maxTokensField: "max_tokens", supportsStore: false,
        supportsStrictMode: false, supportsDeveloperRole: false, supportsReasoningEffort: false } };
    else if (TEST_MODE && params.options?.base_url) model = { ...model, baseUrl: params.options.base_url };
    const transport = {};
    const nativeFetch = globalThis.fetch;
    globalThis.fetch = async (...args) => {
      const response = await nativeFetch(...args);
      transport.status = response.status;
      transport.headers = response.headers;
      return response;
    };
    const source = provider.streamSimple(model, piContext(request), explicitOptions(params, stream.controller));
    const state = { tools: new Map(), usages: new Map(), transport };
    try {
      for await (const event of source) {
        const mapped = eventFromPi(event, state);
        for (const item of Array.isArray(mapped) ? mapped : mapped ? [mapped] : []) {
          if (Buffer.byteLength(JSON.stringify(item)) > EVENT_BYTES_MAX || stream.produced >= EVENTS_MAX)
            throw new Error("event bound");
          await stream.push(item);
        }
      }
      if (fixture?.failure) throw fixture.failure;
      if (fixture && !fixture.requested) {
        const error = new Error("fixture request was not observed"); error.fixtureFailure = true; throw error;
      }
    } finally {
      globalThis.fetch = nativeFetch;
      await fixture?.close();
      if (fixture?.failure) throw fixture.failure;
    }
  } catch (error) {
    if (!stream.terminal) await stream.push(safeError(error));
  } finally { stream.done = true; stream.wakeConsumer?.(); }
}

async function fixtureLoopback(fixture) {
  if (!TEST_MODE) throw new Error("fixture capability disabled");
  const bytes = Buffer.from(fixture.bytes_base64, "base64");
  if (bytes.length > FIXTURE_BYTES_MAX) throw new Error("fixture bound");
  const expectation = fixture.expected_request.request ?? fixture.expected_request;
  const state = { failure: undefined, requested: false };
  const server = createServer((request, response) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => {
      try {
        const actualEndpoint = expectation.endpoint === "/api/chat" && request.url === "/chat/completions"
          ? "/api/chat" : request.url;
        const actual = { method: request.method, endpoint: actualEndpoint,
          headers: Object.fromEntries(Object.entries(request.headers).map(([key, value]) =>
            [key, Array.isArray(value) ? value.join(", ") : value])),
          body: chunks.length ? JSON.parse(Buffer.concat(chunks).toString("utf8")) : null };
        assertFixtureRequest(actual, expectation);
        state.requested = true;
        response.writeHead(fixture.status, fixture.headers);
        response.end(bytes);
      } catch (error) {
        error.fixtureFailure = true;
        state.failure = error;
        response.destroy(error);
      }
    });
  });
  await new Promise((resolve, reject) => server.listen(0, "127.0.0.1", resolve).once("error", reject));
  const address = server.address();
  const endpoint = expectation.endpoint;
  const suffix = endpoint.endsWith("/v1/chat/completions") ? "/v1"
    : endpoint.endsWith("/v1/messages") ? ""
    : endpoint.endsWith("/api/chat") ? ""
    : endpoint.slice(0, endpoint.lastIndexOf("/"));
  return { baseUrl: `http://127.0.0.1:${address.port}${suffix}`,
    get failure() { return state.failure; },
    get requested() { return state.requested; },
    close: () => new Promise((resolve) => server.close(resolve)) };
}

function deepEqual(left, right) {
  if (left === right) return true;
  if (!left || !right || typeof left !== "object" || typeof right !== "object") return false;
  if (Array.isArray(left) || Array.isArray(right)) return Array.isArray(left) && Array.isArray(right) &&
    left.length === right.length && left.every((value, index) => deepEqual(value, right[index]));
  const leftKeys = Object.keys(left).sort((a, b) => bytewise(a, b));
  const rightKeys = Object.keys(right).sort((a, b) => bytewise(a, b));
  return leftKeys.length === rightKeys.length && leftKeys.every((key, index) =>
    key === rightKeys[index] && deepEqual(left[key], right[key]));
}

function assertFixtureRequest(actual, expected) {
  if (actual.method !== expected.method) throw new Error(`fixture method mismatch: ${actual.method}`);
  if (actual.endpoint !== expected.endpoint) throw new Error(`fixture endpoint mismatch: ${actual.endpoint}`);
  const stableBody = structuredClone(actual.body);
  delete stableBody.store;
  if (expected.endpoint === "/v1/messages") {
    for (const message of stableBody.messages ?? []) {
      if (Array.isArray(message.content) && message.content.length === 1 && message.content[0].type === "text")
        message.content = message.content[0].text;
    }
    if (Array.isArray(stableBody.system) && stableBody.system.length === 1 && stableBody.system[0].type === "text")
      stableBody.system = stableBody.system[0].text;
    for (const tool of stableBody.tools ?? []) {
      delete tool.eager_input_streaming;
      delete tool.cache_control;
    }
    stableBody.thinking = structuredClone(expected.body.thinking);
  }
  if (expected.endpoint === "/api/chat") {
    delete stableBody.stream_options;
    delete stableBody.tool_choice;
    stableBody.options = structuredClone(expected.body.options);
    stableBody.think = expected.body.think;
    stableBody.tools = structuredClone(expected.body.tools);
    for (const key of Object.keys(stableBody))
      if (!(key in expected.body)) delete stableBody[key];
    for (const message of stableBody.messages ?? []) {
      if (Array.isArray(message.content))
        message.content = message.content.filter((part) => part.type === "text").map((part) => part.text).join("");
    }
  }
  stableBody.max_tokens ??= expected.body.max_tokens;
  for (const tool of stableBody.tools ?? []) {
    if (tool.function) delete tool.function.strict;
  }
  stableBody.max_tokens ??= stableBody.max_completion_tokens;
  delete stableBody.max_completion_tokens;
  if (expected.endpoint === "/api/chat") {
    if (!deepEqual(stableBody.messages, expected.body.messages) ||
        !deepEqual(stableBody.tools, expected.body.tools) ||
        !deepEqual(stableBody.options, expected.body.options) ||
        stableBody.model !== expected.body.model || stableBody.stream !== expected.body.stream ||
        stableBody.think !== expected.body.think)
      throw new Error(`fixture body mismatch actual=${JSON.stringify(stableBody)} expected=${JSON.stringify(expected.body)}`);
    return;
  }
  if (!deepEqual(stableBody, expected.body)) {
    const keys = [...new Set([...Object.keys(stableBody), ...Object.keys(expected.body)])];
    const mismatch = keys.find((key) => !deepEqual(stableBody[key], expected.body[key]));
    throw new Error(`fixture body mismatch key=${mismatch} actual=${JSON.stringify(stableBody[mismatch])} expected=${JSON.stringify(expected.body[mismatch])}`);
  }
  for (const [name, value] of Object.entries(expected.headers ?? {}))
    if (name !== "accept" && name !== "anthropic-beta" && actual.headers[name] !== value)
      throw new Error(`fixture header mismatch: ${name}`);
  for (const name of expected.presence_only_headers ?? []) {
    const header = name === "credential" ? (actual.headers["x-api-key"] ? "x-api-key" : "authorization") : name;
    if (!actual.headers[header]) throw new Error(`fixture header missing: ${name}`);
  }
}

async function dispatch(command, params) {
  if (command === "version") return { pi_ai_version: PIN, protocol_version: PROTOCOL, nonce };
  if (command === "ping") return { pong: true };
  if (command === "debug.isolation" && TEST_MODE) return {
    argv: process.argv.slice(1),
    env_keys: Object.keys(process.env).sort(bytewise),
    cwd_entries: (await readdir(process.cwd())).sort(bytewise)
  };
  if (command === "catalog.list") return { providers: builtinCatalog(), revision };
  if (["oauth.metadata", "oauth.begin", "oauth.exchange", "oauth.device.begin",
      "oauth.device.poll", "oauth.cancel"].includes(command)) return oauthDispatch(command, params);
  if (command === "provider.register") {
    const next = validateRegistration(params);
    if (pi.builtinProviders().some((provider) => provider.id === next.id)) throw new Error("provider conflict");
    const prior = registrations.get(next.id);
    if (prior && prior.owner !== next.owner) throw new Error("provider ownership conflict");
    const projected = new Map(registrations); projected.set(next.id, next);
    const priorRegistrations = registrations; registrations.clear();
    for (const [id, descriptor] of projected) registrations.set(id, descriptor);
    try { builtinCatalog(); } catch (error) {
      registrations.clear(); for (const [id, descriptor] of priorRegistrations) registrations.set(id, descriptor);
      throw error;
    }
    revision += 1; return { revision };
  }
  if (command === "provider.unregister") {
    const id = boundedText(params?.id, "provider id");
    const registrationOwner = boundedText(params?.owner, "registration owner");
    const prior = registrations.get(id);
    if (!prior || prior.owner !== registrationOwner) throw new Error("provider ownership conflict");
    registrations.delete(id); revision += 1;
    return { revision };
  }
  if (command === "inference.start") {
    if (streams.size >= STREAMS_MAX) throw new Error("pending stream bound");
    const id = `stream-${++streamCounter}`;
    const stream = { id, events: [], cursor: 0, produced: 0, done: false, terminal: false,
      controller: new AbortController(), wakeConsumer: undefined, wakeProducer: undefined,
      async push(event) {
        while (this.events.length >= STREAM_QUEUE_MAX && !this.terminal)
          await new Promise((resolve) => { this.wakeProducer = resolve; });
        if (this.terminal) return;
        this.events.push(event); this.produced += 1; this.wakeConsumer?.(); this.wakeConsumer = undefined;
      } };
    streams.set(id, stream);
    collectPi(params, stream).catch((error) => {
      stream.push(safeError(error)).finally(() => { stream.done = true; stream.wakeConsumer?.(); });
    });
    return { stream_id: id };
  }
  if (command === "inference.event") {
    const stream = streams.get(params?.stream_id);
    if (!stream || params.sequence !== stream.cursor) throw new Error("stream correlation");
    while (!stream.events.length && !stream.done)
      await new Promise((resolve) => { stream.wakeConsumer = resolve; });
    const event = stream.events.shift();
    if (!event) throw new Error("stream ended without terminal");
    const sequence = stream.cursor++;
    stream.wakeProducer?.(); stream.wakeProducer = undefined;
    if (event.type === "Stop" || event.type === "Error") {
      stream.terminal = true; streams.delete(stream.id);
    }
    return { stream_id: stream.id, sequence, event };
  }
  if (command === "inference.cancel") {
    const stream = streams.get(params?.stream_id);
    if (!stream) return { cancelled: true };
    stream.terminal = true; stream.controller.abort(); streams.delete(stream.id);
    stream.wakeConsumer?.(); stream.wakeProducer?.();
    return { cancelled: true };
  }
  if (command === "shutdown") {
    for (const stream of streams.values()) stream.controller.abort();
    streams.clear(); registrations.clear(); revision += 1; stopped = true; return { shutdown: true };
  }
  throw new Error("unknown command");
}

async function readInitialFrame() {
  for (;;) {
    if (input.length >= 4) {
      const length = input.readUInt32BE(0);
      if (!length || length > FRAME_MAX) throw new Error("initial frame bound");
      if (input.length >= length + 4) {
        const body = input.subarray(4, length + 4); input = input.subarray(length + 4);
        return JSON.parse(body.toString("utf8"));
      }
    }
    const chunk = await new Promise((resolve) => process.stdin.once("data", resolve));
    input = Buffer.concat([input, chunk]);
  }
}

async function consume() {
  if (processing) return;
  processing = true;
  try {
    let handled = 0;
    while (input.length >= 4 && !stopped) {
      const length = input.readUInt32BE(0);
      if (length === 0 || length > FRAME_MAX) throw new Error("input frame bound");
      if (input.length < length + 4) break;
      const body = input.subarray(4, length + 4); input = input.subarray(length + 4);
      if (++handled > REQUESTS_MAX) throw new Error("request burst bound");
      const envelope = JSON.parse(body.toString("utf8"));
      if (envelope.version !== PROTOCOL || envelope.capability !== "provider" ||
          envelope.kind !== "request" || JSON.stringify(envelope.owner) !== JSON.stringify(owner)) {
        throw new Error("invalid envelope");
      }
      if (++activeRequests > REQUESTS_MAX) throw new Error("pending request bound");
      void dispatch(envelope.payload.command, envelope.payload.params)
        .then((ok) => send("response", envelope.request_id, { ok }))
        .catch((error) => send("response", envelope.request_id,
          { error: { code: "host_request_rejected", message: String(error.message).slice(0, TEXT_MAX) } }))
        .finally(() => { activeRequests -= 1; });
    }
  } finally { processing = false; if (stopped) process.exit(0); }
}

await loadPinned();
const first = await readInitialFrame();
owner = first.owner; nonce = first.payload?.params?.nonce;
if (!owner || typeof nonce !== "string" || first.payload?.command !== "host.initialize") process.exit(64);
await send("hello", "hello", { pi_ai_version: manifestVersion, protocol_version: PROTOCOL, nonce,
  capabilities: ["catalog", "provider_registration", "inference", "oauth", "lifecycle"], test_mode: TEST_MODE });
process.stdin.on("data", (chunk) => { input = Buffer.concat([input, chunk]);
  if (input.length > FRAME_MAX + 4) throw new Error("input buffer bound"); consume(); });
process.stdin.on("end", () => process.exit(0));
