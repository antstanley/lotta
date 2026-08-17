// Generated compatibility bridge for the pinned Task 45 TypeScript mod contract.
// Golden reference: .specs/05-tools-and-extensions.md#hooks-and-mods
import process from "node:process";
import { pathToFileURL } from "node:url";

const FRAME_BYTES_MAX = 8 * 1024 * 1024;
const MOD_HOST_PENDING_CALLS_MAX = 128;
const CAPABILITY_TIMEOUT_MS_DEFAULT = 30_000;
const REFUSAL_CODE = -32010;
const [entry, ownerJson, sidecarOwnerJson] = process.argv.slice(2);
if (!entry || !ownerJson || !sidecarOwnerJson) process.exit(64);
const owner = JSON.parse(ownerJson);
const sidecarOwner = JSON.parse(sidecarOwnerJson);
let buffer = Buffer.alloc(0);
let api;
let registrations;
let nextId = 1000000;
const pending = new Map();
let capabilities = new Set();
let conversationHandle;

function envelope(kind, requestId, correlationId, payload, timeoutMs = 1) {
  return { version: 1, owner: sidecarOwner, capability: "mod", timeout_ms: timeoutMs,
    request_id: String(requestId), correlation_id: correlationId, kind, payload };
}
function send(value) {
  const payload = Buffer.from(JSON.stringify(value));
  if (!payload.length || payload.length > FRAME_BYTES_MAX) throw new Error("frame bound");
  const prefix = Buffer.allocUnsafe(4);
  prefix.writeUInt32BE(payload.length);
  process.stdout.write(prefix);
  process.stdout.write(payload);
}
function result(id, value) {
  send(envelope("response", `response-${id}`, String(id), { jsonrpc: "2.0", id: Number(id), ...value }));
}
function attributed(batch) {
  const value = structuredClone(batch ?? {});
  for (const key of ["tools", "commands", "providers", "permissions", "lifecycle_events", "ui_metadata"]) {
    value[key] ??= [];
    for (const item of value[key]) item.owner = owner;
  }
  return value;
}
function sendCancel(id) {
  const cancelId = nextId++;
  send(envelope("request", cancelId, null, { jsonrpc: "2.0", id: cancelId, method: "cancel", params: {
    type: "cancel", owner, request_id: id
  }}));
}
// Every settle path clears the timer and detaches the abort listener exactly once, so a resolved
// capability call leaves no timer holding the event loop open and no listener retaining its scope.
function settle(id) {
  const waiting = pending.get(id);
  if (!waiting) return undefined;
  pending.delete(id);
  clearTimeout(waiting.timer);
  waiting.release();
  return waiting;
}
function capabilityCall(capability, operation, params, options = {}) {
  if (!capabilities.has(capability)) return Promise.reject(Object.assign(new Error("capability not declared"), { code: REFUSAL_CODE }));
  if (pending.size >= MOD_HOST_PENDING_CALLS_MAX) return Promise.reject(Object.assign(new Error("pending capability limit"), { code: REFUSAL_CODE }));
  const id = nextId++;
  const timeoutMs = Math.max(1, Math.min(Number(options.timeoutMs ?? CAPABILITY_TIMEOUT_MS_DEFAULT), CAPABILITY_TIMEOUT_MS_DEFAULT));
  const request = { jsonrpc: "2.0", id, method: "capability.call", params: {
    type: "capability_call", owner, conversation_handle: conversationHandle,
    capability, operation, params
  }};
  send(envelope("request", id, null, request, timeoutMs));
  return new Promise((resolve, reject) => {
    const signal = options.signal;
    const onAbort = () => {
      if (!settle(id)) return;
      sendCancel(id);
      reject(new Error("capability cancelled"));
    };
    const timer = setTimeout(() => {
      if (!settle(id)) return;
      sendCancel(id);
      reject(new Error("capability timed out"));
    }, timeoutMs);
    timer.unref?.();
    const release = () => signal?.removeEventListener("abort", onAbort);
    pending.set(id, { resolve, reject, timer, release });
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}
function rejectPending(message) {
  for (const id of [...pending.keys()]) {
    const waiting = settle(id);
    waiting?.reject(new Error(message));
  }
}
function shutdownBridge(code) {
  rejectPending("mod host stopped");
  process.stdin.removeListener("data", onData);
  process.stdin.removeListener("end", onEnd);
  process.removeListener("SIGTERM", onSigterm);
  process.stdin.pause();
  process.exit(code);
}
function registrationApi() {
  const batch = { tools: [], commands: [], providers: [], permissions: [], lifecycle_events: [], ui_metadata: [] };
  return {
    batch,
    registerTool: value => batch.tools.push(value),
    registerCommand: value => batch.commands.push(value),
    registerProvider: value => batch.providers.push(value),
    registerPermission: value => batch.permissions.push(value),
    registerLifecycleEvent: value => batch.lifecycle_events.push(value),
    registerUiMetadata: value => batch.ui_metadata.push(value),
    capability: capabilityCall,
    conversationHandle: () => conversationHandle,
  };
}
async function handleRequest(message) {
  const request = message.payload;
  const id = request.id;
  try {
    switch (request.method) {
      case "initialize": {
        capabilities = new Set(request.params.capabilities ?? []);
        conversationHandle = request.params.conversation_handle;
        const loaded = await import(pathToFileURL(entry).href);
        api = loaded.default ?? loaded.mod ?? loaded;
        const registration = registrationApi();
        const returned = await api.register?.(registration, request.params);
        registrations = attributed(returned ?? api.registrations ?? registration.batch);
        result(id, { result: { type: "initialized" } });
        break;
      }
      case "register": result(id, { result: { type: "registration_batch", registrations } }); break;
      case "tool.call": result(id, { result: { type: "value", value:
        await api.toolCall(request.params.name, request.params.input, request.params.tool_call_id) } }); break;
      case "command.call": result(id, { result: { type: "value", value:
        await api.commandCall(request.params.name, request.params.arguments) } }); break;
      case "lifecycle.call": result(id, { result: { type: "value", value:
        await api.lifecycleCall?.(request.params.name, request.params.payload) ?? null } }); break;
      case "diagnostics": result(id, { result: { type: "diagnostics", diagnostics: api.diagnostics?.() ?? [] } }); break;
      case "reload": { await api.reload?.(); result(id, { result: { type: "reloaded" } }); break; }
      case "cancel": { await api.cancel?.(request.params.request_id); result(id, { result: { type: "cancelled" } }); break; }
      case "dispose": { rejectPending("mod disposed"); await api.dispose?.(); result(id, { result: { type: "disposed" } }); setImmediate(() => shutdownBridge(0)); break; }
      default: result(id, { error: { code: -32601, message: "unknown method" } });
    }
  } catch (_) {
    result(id, { error: { code: -32000, message: "mod operation failed" } });
  }
}
function handleResponse(message) {
  const response = message.payload;
  const waiting = settle(response.id);
  if (!waiting) return;
  if (response.error) waiting.reject(Object.assign(new Error("capability failed"), { code: response.error.code }));
  else waiting.resolve(response.result?.value ?? response.result);
}
async function handle(message) {
  if (message.kind === "request") await handleRequest(message);
  else if (message.kind === "response") handleResponse(message);
}
function onData(chunk) {
  if (chunk.length > FRAME_BYTES_MAX + 4 || buffer.length > FRAME_BYTES_MAX + 4 - chunk.length) shutdownBridge(65);
  const next = Buffer.allocUnsafe(buffer.length + chunk.length);
  buffer.copy(next);
  chunk.copy(next, buffer.length);
  buffer = next;
  while (buffer.length >= 4) {
    const length = buffer.readUInt32BE(0);
    if (!length || length > FRAME_BYTES_MAX) shutdownBridge(65);
    if (buffer.length < 4 + length) break;
    let frame;
    try { frame = JSON.parse(buffer.subarray(4, 4 + length)); } catch (_) { shutdownBridge(65); }
    buffer = buffer.subarray(4 + length);
    void handle(frame);
  }
}
function onEnd() { shutdownBridge(0); }
function onSigterm() { shutdownBridge(0); }
process.stdin.on("data", onData);
process.stdin.on("end", onEnd);
process.on("SIGTERM", onSigterm);
send(envelope("hello", 1, null, { protocol_version: 1, owner: sidecarOwner,
  capabilities: ["mod"], timeout_ms_max: 300000, frame_bytes_max: FRAME_BYTES_MAX }));
