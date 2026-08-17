import { spawn } from "bun";

const FRAME_MAX = 4 * 1024 * 1024;
const OUTPUT_MAX = 16 * 1024 * 1024;
const VERSION = 1;
const CAPABILITY = "subagent";

type Envelope = {
  version: number;
  owner: { agent_id: string; runtime_id: string; conversation_id: string };
  capability: string;
  timeout_ms: number;
  request_id: string;
  correlation_id: string | null;
  kind: string;
  payload: Record<string, unknown>;
};

function option(name: string): string {
  const index = Bun.argv.indexOf(name);
  const value = index >= 0 ? Bun.argv[index + 1] : undefined;
  if (!value || value.includes("\0")) throw new Error("invalid adapter option");
  return value;
}

class FrameDecoder {
  private pending = new Uint8Array(0);
  constructor(private reader: ReadableStreamDefaultReader<Uint8Array>) {}
  private async fill(length: number): Promise<void> {
    while (this.pending.length < length) {
      const { value, done } = await this.reader.read();
      if (done || !value) throw new Error("truncated Task42 frame");
      if (value.length > FRAME_MAX + 4) throw new Error("Task42 chunk bound");
      const next = new Uint8Array(this.pending.length + value.length);
      next.set(this.pending); next.set(value, this.pending.length); this.pending = next;
    }
  }
  async frame(): Promise<Envelope> {
    await this.fill(4);
    const length = new DataView(this.pending.buffer, this.pending.byteOffset, 4).getUint32(0, false);
    if (length === 0 || length > FRAME_MAX) throw new Error("Task42 frame bound");
    await this.fill(4 + length);
    const bytes = this.pending.slice(4, 4 + length);
    this.pending = this.pending.slice(4 + length);
    return JSON.parse(new TextDecoder().decode(bytes)) as Envelope;
  }
}

function validateHello(value: Envelope): void {
  if (
    value.version !== VERSION || value.kind !== "hello" ||
    value.capability !== CAPABILITY || value.timeout_ms <= 0 ||
    !value.owner?.agent_id || !value.owner.runtime_id || !value.owner.conversation_id ||
    !value.request_id || value.correlation_id !== null
  ) throw new Error("invalid Task42 hello");
}

function validateRequest(value: Envelope, hello: Envelope): void {
  if (
    value.version !== VERSION || value.kind !== "request" ||
    value.capability !== CAPABILITY || value.timeout_ms !== hello.timeout_ms ||
    JSON.stringify(value.owner) !== JSON.stringify(hello.owner) ||
    value.request_id !== hello.request_id || value.correlation_id !== hello.request_id
  ) throw new Error("invalid Task42 request");
}

function args(request: any, sourceRoot: string): string[] {
  const output = [
    "run", `${sourceRoot}/src/index.ts`, "--backend", "local",
    "--input-format", "stream-json", "--output-format", "stream-json",
    ...(process.env.LOTTA_TASK46_DEV_BACKEND ? ["--dev-backend", process.env.LOTTA_TASK46_DEV_BACKEND] : []),
    "--include-partial-messages",
  ];
  if (request.existing_conversation_id) output.push("--conv", request.existing_conversation_id);
  else if (request.existing_agent_id) output.push("--agent", request.existing_agent_id, "--new");
  else output.push("--new-agent", "--system", "default");
  if (request.model?.policy === "explicit" || request.model?.policy === "inherit") {
    output.push("--model", request.model.model);
  } else if (request.model?.policy === "auto_fast") output.push("--model", "haiku-4.5");
  if (request.tools !== "all" && request.tools?.only) output.push("--tools", request.tools.only.join(","));
  if (request.max_turns) output.push("--max-turns", String(request.max_turns));
  if (request.type === "reflection") output.push("--no-system-info-reminder", "--no-skills");
  if (["reflection", "memory", "history-analyzer", "init"].includes(request.type)) {
    output.push("--base-tools", "none");
  }
  return output;
}

function response(base: Envelope, payload: unknown, kind = "event"): Envelope {
  return { ...base, kind, correlation_id: base.request_id, payload: payload as Record<string, unknown> };
}

function write(value: Envelope): void {
  const bytes = new TextEncoder().encode(JSON.stringify(value));
  if (bytes.length === 0 || bytes.length > FRAME_MAX) throw new Error("Task42 output frame bound");
  const frame = Buffer.allocUnsafe(4 + bytes.length);
  frame.writeUInt32BE(bytes.length, 0);
  frame.set(bytes, 4);
  process.stdout.write(frame);
}


const bun = option("--bun");
const sourceRoot = option("--source-root");
const cwd = option("--cwd");
const decoder = new FrameDecoder(Bun.stdin.stream().getReader());
const hello = await decoder.frame();
validateHello(hello);
write(hello);
const request = await decoder.frame();
validateRequest(request, hello);
const child = Bun.spawn({
  cmd: [bun, ...args(request.payload, sourceRoot)], cwd,
  env: { ...process.env, LETTA_CODE_AGENT_ROLE: "subagent" },
  stdin: "pipe", stdout: "pipe", stderr: "pipe",
});
const stderrPromise = new Response(child.stderr).arrayBuffer();
const abort = () => { child.kill("SIGKILL"); };
process.on("SIGTERM", abort);
process.on("SIGINT", abort);
const prompt = (request.payload as any).prompt;
const context = (request.payload as any).resolved_context;
const roots = (request.payload as any).filesystem_roots ?? [];
const memory = (request.payload as any).memory_scope;
const visibleMemory = [...(memory?.readonly_roots ?? []), ...(memory?.writable_roots ?? [])];
if (process.env.LOTTA_TASK46_MARKER_PROBE) {
  const probe = JSON.parse(process.env.LOTTA_TASK46_MARKER_PROBE) as {
    inside: string; absolute: string; parent: string; symlink: string; primary: string;
  };
  const results: Record<string, boolean> = {};
  for (const [name, path] of Object.entries(probe)) {
    try {
      await Bun.file(path).text();
      results[name] = true;
    } catch {
      results[name] = false;
    }
  }
  process.stderr.write(`TASK46_MARKER_PROBE=${JSON.stringify(results)}\n`);
}
const metadata = JSON.stringify({
  description: (request.payload as any).description,
  runtime_id: (request.payload as any).parent_scope?.runtime_id,
  parent_scope: (request.payload as any).parent_scope,
  background: (request.payload as any).background,
  filesystem_roots: roots,
  memory_roots: visibleMemory,
  reflection_worktree: (request.payload as any).reflection_worktree,
});
const content = `${prompt}\n\n<subagent-metadata>${metadata}</subagent-metadata>` +
  (context ? `\n\n<parent-context>${context}</parent-context>` : "");
child.stdin.write(`${JSON.stringify({ type: "user", message: { content } })}\n`);
child.stdin.end();
let total = 0;
let pending = "";
const textDecoder = new TextDecoder();
try {
for await (const chunk of child.stdout) {
  if (chunk.length > FRAME_MAX) { abort(); throw new Error("Letta chunk bound"); }
  total += chunk.length;
  if (total > OUTPUT_MAX) { abort(); throw new Error("Letta output bound"); }
  const decoded = textDecoder.decode(chunk, { stream: true });
  if (Buffer.byteLength(pending) + Buffer.byteLength(decoded) > FRAME_MAX) {
    abort(); throw new Error("Letta line bound");
  }
  pending += decoded;
  while (pending.includes("\n")) {
    const index = pending.indexOf("\n");
    const line = pending.slice(0, index);
    pending = pending.slice(index + 1);
    if (!line.trim()) continue;
    const event = JSON.parse(line);
    write(response(request, event, event.type === "result" ? "response" : "event"));
  }
}
const exitCode = await child.exited;
if (pending.trim()) write(response(request, JSON.parse(pending)));
if (exitCode !== 0) {
  const stderr = new TextDecoder().decode(await stderrPromise);
  throw new Error(`Letta process failed: ${stderr.slice(0, 4096)}`);
}
} finally {
  if ((await Promise.race([child.exited.then(() => true), Bun.sleep(0).then(() => false)])) === false) abort();
  await child.exited;
  await stderrPromise;
}
