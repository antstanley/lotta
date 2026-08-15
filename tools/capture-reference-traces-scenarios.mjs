import { createHash } from "node:crypto";
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

import {
  BOUNDARY,
  LIMITS,
  PLACEHOLDER,
  SUPPORT,
} from "./capture-reference-traces-data.mjs";

function fail(message) {
  throw new Error(`reference trace capture failed: ${message}`);
}
function sha(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
export class ReferenceSocket {
  static instances = [];
  readyState = 0;
  listeners = new Map();
  sent = [];
  constructor() {
    ReferenceSocket.instances.push(this);
  }
  addEventListener(type, listener) {
    const values = this.listeners.get(type) ?? [];
    values.push(listener);
    this.listeners.set(type, values);
  }
  removeEventListener(type, listener) {
    this.listeners.set(
      type,
      (this.listeners.get(type) ?? []).filter((x) => x !== listener),
    );
  }
  send(data) {
    const wire = JSON.parse(data);
    this.sent.push(wire);
    this.onCommand?.(wire);
  }
  close() {
    this.readyState = 3;
    this.emit("close", { explicit: true });
  }
  open() {
    this.readyState = 1;
    this.emit("open", {});
  }
  message(value) {
    this.emit("message", { data: JSON.stringify(value) });
  }
  disconnect(reason = "unexpected") {
    this.readyState = 3;
    this.emit("close", { reason });
  }
  emit(type, event) {
    for (const listener of this.listeners.get(type) ?? [])
      listener(event);
  }
}
export class Recorder {
  frames = [];
  seq = new Map();
  time = 1;
  emission = 1;
  requests = new Map();
  accepted = new Set();
  lease = new Map();
  lifecycle(connection, type, fields = {}, metadata = {}) {
    this.frames.push({
      direction: "lifecycle",
      connection_ordinal: connection,
      ...(metadata.causedBy ? { caused_by: metadata.causedBy } : {}),
      ...(metadata.lease ? { lease_id: metadata.lease } : {}),
      wire: { type, ...fields },
    });
  }
  sent(connection, wire, socket) {
    queueMicrotask(() => {
      const actual = socket.sent.at(-1);
      if (JSON.stringify(actual) !== JSON.stringify(wire))
        fail("onSend/socket exact mismatch");
    });
    const causedBy = inputIdentity(wire);
    if (wire.request_id && causedBy)
      this.requests.set(wire.request_id, causedBy);
    this.frames.push({
      direction: "client_to_server",
      connection_ordinal: connection,
      ...(causedBy ? { caused_by: causedBy } : {}),
      wire: structuredClone(wire),
    });
  }
  received(connection, wire, metadata = {}) {
    let causedBy = metadata.causedBy;
    if (wire.type === "input_accepted") {
      causedBy = this.requests.get(wire.request_id);
      if (!causedBy)
        fail("input acceptance has no observed request cause");
      this.accepted.add(causedBy);
    }
    if (
      causedBy &&
      !this.accepted.has(causedBy) &&
      wire.type !== "input_accepted"
    ) {
      fail("server event precedes input acceptance");
    }
    this.frames.push({
      direction: "server_to_client",
      connection_ordinal: connection,
      ...(metadata.emission
        ? { broadcast_emission: metadata.emission }
        : {}),
      ...(causedBy ? { caused_by: causedBy } : {}),
      ...(metadata.lease
        ? {
            lease_id: metadata.lease,
            lease_current: metadata.current !== false,
          }
        : {}),
      wire: structuredClone(wire),
    });
  }
}
function inputIdentity(wire) {
  if (wire.type !== "input") return undefined;
  const messages = wire.payload?.messages ?? [];
  return messages.find(
    (value) => typeof value.client_message_id === "string",
  )?.client_message_id;
}
function logicalPayload(value) {
  if (Array.isArray(value)) return value.map(logicalPayload);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(
    Object.entries(value)
      .filter(
        ([key]) =>
          ![
            "runtime",
            "event_seq",
            "emitted_at",
            "idempotency_key",
          ].includes(key),
      )
      .map(([key, child]) => [key, logicalPayload(child)]),
  );
}
function canonicalJson(value) {
  if (Array.isArray(value))
    return `[${value.map(canonicalJson).join(",")}]`;
  if (!value || typeof value !== "object") return JSON.stringify(value);
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(",")}}`;
}
export class ScenarioHarness {
  constructor(module) {
    this.module = module;
    this.recorder = new Recorder();
    this.sessions = new Map();
  }
  async session(ordinal) {
    const client = new this.module.AppServerClient({
      url: "ws://127.0.0.1:1",
      WebSocket: ReferenceSocket,
      requestTimeoutMs: 1000,
    });
    const socket = ReferenceSocket.instances.at(-1),
      messages = [],
      disconnects = [];
    client.onSend((wire) => this.recorder.sent(ordinal, wire, socket));
    client.onMessage((wire) => {
      messages.push(structuredClone(wire));
      this.recorder.received(
        ordinal,
        wire,
        this.deliveryMetadata ?? {},
      );
    });
    client.onDisconnect((event) => {
      disconnects.push(event.channel);
      this.recorder.lifecycle(ordinal, "disconnect", {
        channel: event.channel,
      });
    });
    socket.onCommand = (wire) => this.onCommand?.(ordinal, wire);
    socket.open();
    await client.connect();
    this.recorder.lifecycle(ordinal, "open");
    const session = { ordinal, client, socket, messages, disconnects };
    this.sessions.set(ordinal, session);
    return session;
  }
  deliver(ordinal, wire, metadata = {}) {
    const session = this.sessions.get(ordinal);
    if (!session) fail("delivery session missing");
    const before = session.messages.length;
    this.deliveryMetadata = metadata;
    session.socket.message(wire);
    this.deliveryMetadata = undefined;
    if (session.messages.length !== before + 1)
      fail("real onMessage not observed");
  }
  respond(ordinal, wire) {
    this.deliver(ordinal, wire);
  }
  startTurn(connection, causedBy, lease, dequeued = false) {
    const type = dequeued ? "turn_dequeued" : "turn_started";
    this.recorder.lifecycle(
      connection,
      type,
      { type, caused_by: causedBy, lease_id: lease },
      { causedBy, lease },
    );
    this.recorder.lifecycle(
      connection,
      "lease_acquired",
      { lease_id: lease, caused_by: causedBy },
      { causedBy, lease },
    );
    this.recorder.lease.set(causedBy, lease);
  }
  replaceLease(connection, causedBy, oldLease, newLease) {
    this.recorder.lifecycle(
      connection,
      "lease_replaced",
      {
        old_lease_id: oldLease,
        new_lease_id: newLease,
        caused_by: causedBy,
      },
      { causedBy, lease: newLease },
    );
    this.recorder.lifecycle(
      connection,
      "lease_acquired",
      { lease_id: newLease, caused_by: causedBy },
      { causedBy, lease: newLease },
    );
    this.recorder.lease.set(causedBy, newLease);
  }
  broadcast(type, ordinals, fields, metadata = {}) {
    const emission =
      metadata.emission ?? `emission-${this.recorder.emission++}`;
    const logical = { type, ...fields };
    this.recorder.lifecycle(
      ordinals[0],
      "broadcast_begin",
      {
        type: "broadcast_begin",
        emission,
        message_type: type,
        subscriber_ordinals: [...ordinals],
        payload_sha256: sha(canonicalJson(logicalPayload(logical))),
      },
      metadata,
    );
    for (const ordinal of [...ordinals].sort((a, b) => a - b)) {
      const seq = (this.recorder.seq.get(ordinal) ?? 0) + 1;
      this.recorder.seq.set(ordinal, seq);
      const wire = {
        ...structuredClone(logical),
        runtime: runtimeScope(),
        event_seq: seq,
        emitted_at: `2000-01-01T00:00:${String(
          this.recorder.time++,
        ).padStart(2, "0")}.000Z`,
        idempotency_key: `${type}:${seq}:${uuid(100 + this.recorder.time)}`,
      };
      this.deliver(ordinal, wire, { ...metadata, emission });
    }
  }
}
function standardResponse(command, disposition = "started") {
  const runtime = runtimeScope();
  if (command.type === "runtime_start")
    return {
      type: "runtime_start_response",
      request_id: command.request_id,
      success: true,
      runtime,
      agent: null,
      conversation: null,
      created: { agent: false, conversation: false },
    };
  if (command.type === "input")
    return {
      type: "input_accepted",
      request_id: command.request_id,
      runtime,
      accepted: true,
      disposition,
    };
  if (command.type === "abort_message")
    return {
      type: "abort_message_response",
      request_id: command.request_id,
      runtime,
      aborted: true,
      success: true,
    };
  if (command.type === "sync")
    return {
      type: "sync_response",
      request_id: command.request_id,
      runtime,
      success: true,
    };
  fail(`unsupported command: ${command.type}`);
}
async function importPinnedClient(sourceRoot) {
  const temp = mkdtempSync(
    resolve(tmpdir(), "lotta-reference-client-"),
  );
  mkdirSync(resolve(temp, "types"));
  const source = readFileSync(
    resolve(sourceRoot, "src/app-server-client.ts"),
    "utf8",
  );
  writeFileSync(
    resolve(temp, "app-server-client.ts"),
    source.replaceAll(
      '"./types/app-server-info"',
      '"./types/app-server-info.ts"',
    ),
  );
  cpSync(
    resolve(sourceRoot, "src/types/app-server-info.ts"),
    resolve(temp, "types/app-server-info.ts"),
  );
  try {
    return await import(
      `${pathToFileURL(resolve(temp, "app-server-client.ts"))}?pin=1`
    );
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}
function uuid(number) {
  return `00000000-0000-4000-8000-${String(number).padStart(12, "0")}`;
}
function runtimeScope() {
  return { agent_id: uuid(1), conversation_id: uuid(2) };
}
function inputPayload(id) {
  return {
    kind: "create_message",
    messages: [
      { role: "user", content: PLACEHOLDER, client_message_id: id },
    ],
  };
}
function deltaFields(index, kind = "message", extra = {}) {
  return {
    delta: {
      id: uuid(10 + index),
      date: `2000-01-01T00:00:${String(index).padStart(2, "0")}.000Z`,
      message_type: kind,
      ...extra,
    },
  };
}
async function startRuntime(
  h,
  session,
  request = `request-runtime-${session.ordinal}`,
) {
  h.onCommand = (ordinal, command) =>
    h.respond(ordinal, standardResponse(command));
  await session.client.runtimeStart({
    request_id: request,
    ...runtimeScope(),
  });
}
async function submit(
  h,
  session,
  id,
  request,
  disposition = "started",
) {
  h.onCommand = (ordinal, command) =>
    h.respond(ordinal, standardResponse(command, disposition));
  return session.client.submitInput({
    request_id: request,
    runtime: runtimeScope(),
    payload: inputPayload(id),
  });
}
function finish(h, ordinals, causedBy, lease, stop = "end_turn") {
  h.broadcast(
    "turn_finished",
    ordinals,
    { turn_id: uuid(90), run_id: uuid(91), stop_reason: stop },
    { causedBy, lease },
  );
}
async function queueScenario(module) {
  ReferenceSocket.instances.length = 0;
  const h = new ScenarioHarness(module),
    c = await h.session(1);
  await startRuntime(h, c);
  const first = "client-message-queue-1",
    second = "client-message-queue-2";
  await submit(h, c, first, "request-queue-1");
  h.startTurn(1, first, "lease-queue-1");
  h.broadcast("stream_delta", [1], deltaFields(1), {
    causedBy: first,
    lease: "lease-queue-1",
  });
  await submit(h, c, second, "request-queue-2", "queued");
  h.broadcast("update_queue", [1], {
    queue: [
      {
        id: uuid(30),
        client_message_id: second,
        enqueued_at: "2000-01-01T00:00:03.000Z",
      },
    ],
    removed: [],
  });
  finish(h, [1], first, "lease-queue-1");
  h.broadcast("update_queue", [1], {
    queue: [],
    removed: [{ client_message_id: second, disposition: "dequeued" }],
  });
  h.startTurn(1, second, "lease-queue-2", true);
  h.broadcast("stream_delta", [1], deltaFields(2), {
    causedBy: second,
    lease: "lease-queue-2",
  });
  finish(h, [1], second, "lease-queue-2");
  return makeTrace(
    "queue",
    "queue",
    h,
    "src/websocket/listener/queue-update-transitions.test.ts",
    "test:active continuation dequeue emits exact message identities",
  );
}
async function abortScenario(module) {
  ReferenceSocket.instances.length = 0;
  const h = new ScenarioHarness(module),
    c = await h.session(1);
  await startRuntime(h, c);
  const id = "client-message-abort";
  await submit(h, c, id, "request-abort-input");
  h.startTurn(1, id, "lease-abort");
  h.onCommand = (ordinal, command) =>
    h.respond(ordinal, standardResponse(command));
  await c.client.abort({
    request_id: "request-abort",
    runtime: runtimeScope(),
    run_id: uuid(40),
  });
  h.broadcast(
    "stream_delta",
    [1],
    deltaFields(3, "message", { type: "message", delta: PLACEHOLDER }),
    { causedBy: id, lease: "lease-abort" },
  );
  finish(h, [1], id, "lease-abort", "abort");
  return makeTrace(
    "abort",
    "abort",
    h,
    "src/websocket/listener/turn-terminal-protocol.test.ts",
    "test:finishListenerTurn emits exactly one correlated terminal event",
  );
}
async function disconnectScenario(module) {
  ReferenceSocket.instances.length = 0;
  const h = new ScenarioHarness(module),
    c1 = await h.session(1);
  await startRuntime(h, c1);
  const active = "client-message-disconnect-active";
  const queued = "client-message-disconnect-queued";
  await submit(h, c1, active, "request-disconnect-1");
  h.startTurn(1, active, "lease-disconnect");
  await submit(h, c1, queued, "request-disconnect-2", "queued");
  c1.socket.disconnect();
  h.recorder.lifecycle(1, "connection_cleanup", {
    cancelled: [queued],
  });
  const c2 = await h.session(2);
  await startRuntime(h, c2, "request-reconnect-runtime");
  h.onCommand = (ordinal, command) =>
    h.respond(ordinal, standardResponse(command));
  await c2.client.sync({
    request_id: "request-reconnect-sync",
    runtime: runtimeScope(),
    recover_approvals: false,
  });
  h.broadcast("update_queue", [2], {
    queue: [],
    removed: [{ client_message_id: queued, disposition: "cancelled" }],
  });
  h.broadcast("stream_delta", [2], deltaFields(4), {
    causedBy: active,
    lease: "lease-disconnect",
  });
  finish(h, [2], active, "lease-disconnect");
  if (c1.messages.some((x) => x.type === "stream_delta"))
    fail("disconnected client received continuation");
  return makeTrace(
    "disconnect",
    "disconnect",
    h,
    "src/websocket/listener/connection-lifecycle.test.ts",
    "test:connection cleanup preserves other subscribers",
  );
}
async function staleScenario(module) {
  ReferenceSocket.instances.length = 0;
  const h = new ScenarioHarness(module),
    c = await h.session(1);
  await startRuntime(h, c);
  const id = "client-message-stale";
  await submit(h, c, id, "request-stale");
  h.startTurn(1, id, "lease-old");
  h.broadcast("stream_delta", [1], deltaFields(5), {
    causedBy: id,
    lease: "lease-old",
  });
  h.replaceLease(1, id, "lease-old", "lease-new");
  h.broadcast("stream_delta", [1], deltaFields(6), {
    causedBy: id,
    lease: "lease-new",
  });
  finish(h, [1], id, "lease-new");
  return makeTrace(
    "stale-lease",
    "stale-lease",
    h,
    "src/websocket/listener/send-lease.test.ts",
    "test:a reset during tool preparation cannot consume replacement input",
  );
}
async function retryScenario(module) {
  ReferenceSocket.instances.length = 0;
  const h = new ScenarioHarness(module),
    c = await h.session(1);
  await startRuntime(h, c);
  const id = "client-message-retry";
  await submit(h, c, id, "request-retry");
  h.startTurn(1, id, "lease-retry");
  h.broadcast(
    "stream_delta",
    [1],
    deltaFields(7, "retry", {
      message: PLACEHOLDER,
      reason: "api_error",
      attempt: 1,
      max_attempts: 3,
      delay_ms: 250,
      retry_kind: "provider_retry",
    }),
    { causedBy: id, lease: "lease-retry" },
  );
  h.broadcast(
    "stream_delta",
    [1],
    deltaFields(8, "message", { type: "message", delta: PLACEHOLDER }),
    { causedBy: id, lease: "lease-retry" },
  );
  finish(h, [1], id, "lease-retry");
  return makeTrace(
    "retry",
    "retry",
    h,
    "src/websocket/listener/cloud-retry-message.test.ts",
    "test:normalizes Cloud retry metadata into the listener protocol",
  );
}
async function idempotencyScenario(module) {
  ReferenceSocket.instances.length = 0;
  const h = new ScenarioHarness(module),
    c = await h.session(1);
  await startRuntime(h, c);
  const id = "client-message-idempotent";
  let admitted = false;
  h.onCommand = (ordinal, command) => {
    h.respond(ordinal, standardResponse(command, "started"));
    if (command.type === "input" && !admitted) {
      admitted = true;
      h.startTurn(1, id, "lease-idempotent");
    }
  };
  await c.client.submitInput({
    request_id: "request-idempotency-1",
    runtime: runtimeScope(),
    payload: inputPayload(id),
  });
  await c.client.submitInput({
    request_id: "request-idempotency-2",
    runtime: runtimeScope(),
    payload: inputPayload(id),
  });
  h.broadcast("stream_delta", [1], deltaFields(9), {
    causedBy: id,
    lease: "lease-idempotent",
  });
  finish(h, [1], id, "lease-idempotent");
  return makeTrace(
    "idempotency",
    "idempotency",
    h,
    "src/websocket/listener/turn-input-state.test.ts",
    "describe:listener turn input state",
  );
}
async function crashScenario(module) {
  ReferenceSocket.instances.length = 0;
  const h = new ScenarioHarness(module),
    c1 = await h.session(1);
  await startRuntime(h, c1);
  const id = "client-message-recovery";
  await submit(h, c1, id, "request-recovery-input");
  h.startTurn(1, id, "lease-before-crash");
  h.broadcast("stream_delta", [1], deltaFields(10), {
    causedBy: id,
    lease: "lease-before-crash",
  });
  c1.socket.disconnect("process_crash");
  h.recorder.lifecycle(1, "process_crash");
  h.recorder.lifecycle(2, "restart");
  const c2 = await h.session(2);
  await startRuntime(h, c2, "request-recovery-runtime");
  h.onCommand = (ordinal, command) =>
    h.respond(ordinal, standardResponse(command));
  await c2.client.sync({
    request_id: "request-recovery-sync",
    runtime: runtimeScope(),
    recover_approvals: false,
  });
  h.replaceLease(2, id, "lease-before-crash", "lease-recovered");
  h.broadcast("stream_delta", [2], deltaFields(11), {
    causedBy: id,
    lease: "lease-recovered",
  });
  finish(h, [2], id, "lease-recovered");
  return makeTrace(
    "crash-recovery",
    "crash-recovery",
    h,
    "src/websocket/listener/recovery-sync.test.ts",
    "describe:recoverApprovalStateForSync restart recovery",
  );
}
async function verticalScenario(module) {
  ReferenceSocket.instances.length = 0;
  const h = new ScenarioHarness(module), c1 = await h.session(1);
  await startRuntime(h, c1);
  h.broadcast("update_device_status", [1], {
    device_status: { mode: "standard", cwd: "/synthetic/workspace" },
  });
  h.broadcast("update_loop_status", [1], {
    loop_status: {
      status: "IDLE",
      active_run_ids: [],
      executing_tool_call_ids: [],
    },
  });
  h.broadcast("update_queue", [1], { queue: [], removed: [] });
  const id = "client-message-slice";
  await submit(h, c1, id, "request-slice");
  h.startTurn(1, id, "lease-slice");
  h.broadcast("stream_delta", [1], deltaFields(12), {
    causedBy: id,
    lease: "lease-slice",
  });
  const c2 = await h.session(2);
  await startRuntime(h, c2, "request-subscriber-runtime");
  h.broadcast(
    "update_loop_status",
    [1, 2],
    {
      loop_status: {
        status: "EXECUTING_CLIENT_SIDE_TOOL",
        active_run_ids: [uuid(80)],
        executing_tool_call_ids: [uuid(81)],
      },
    },
    { causedBy: id },
  );
  h.broadcast(
    "stream_delta",
    [1, 2],
    deltaFields(13, "client_tool_start", {
      tool_call_id: uuid(81),
      tool_name: "synthetic_tool",
      tool_args: PLACEHOLDER,
    }),
    { causedBy: id, lease: "lease-slice" },
  );
  h.broadcast(
    "stream_delta",
    [1, 2],
    deltaFields(14, "client_tool_end", {
      tool_call_id: uuid(81),
      status: "success",
    }),
    { causedBy: id, lease: "lease-slice" },
  );
  h.broadcast(
    "stream_delta",
    [1, 2],
    deltaFields(15, "message", { type: "message", delta: PLACEHOLDER }),
    { causedBy: id, lease: "lease-slice" },
  );
  finish(h, [1, 2], id, "lease-slice");
  return makeTrace(
    "vertical-slice",
    "vertical_slice",
    h,
    "src/app-server-client.test.ts",
    "test:connects one socket and resolves request_id responses",
  );
}
function provenance(path, symbol) {
  return { path, symbol, capture_boundary: BOUNDARY };
}
function makeTrace(name, kind, h, path, symbol) {
  if (h.recorder.frames.length > LIMITS.frames)
    fail(`frame bound: ${name}`);
  const commandTypes = h.recorder.frames
    .filter((x) => x.direction === "client_to_server")
    .map((x) => x.wire.type);
  const messageTypes = h.recorder.frames
    .filter((x) => x.direction === "server_to_client")
    .map((x) => x.wire.type);
  const supporting = SUPPORT[name].map(([p, s]) => provenance(p, s));
  if (!supporting.length || supporting.length > LIMITS.supporting)
    fail(`supporting bound: ${name}`);
  return {
    schema_version: 1,
    name,
    kind,
    provenance: provenance(path, symbol),
    supporting_provenance: supporting,
    driver_proof: {
      command_types: commandTypes,
      message_types: messageTypes,
    },
    frames: h.recorder.frames.map((frame, frameIndex) => ({
      frame_index: frameIndex,
      ...frame,
    })),
  };
}

export async function captureReferenceTraces(sourceRoot) {
  const module = await importPinnedClient(sourceRoot);
  const scenarios = [
    queueScenario,
    abortScenario,
    disconnectScenario,
    staleScenario,
    retryScenario,
    idempotencyScenario,
    crashScenario,
    verticalScenario,
  ];
  const traces = [];
  for (const scenario of scenarios) traces.push(await scenario(module));
  return traces;
}
