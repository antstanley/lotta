import { lstat, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

type Mutation = { value: unknown; changed: boolean; remove: boolean };
type Scan = {
  records: Record<string, Record<string, unknown>>;
  artifacts: string[];
};

export function assertSameStore(
  left: { files: Record<string, string>; provider: number },
  right: { files: Record<string, string>; provider: number },
  phase: string,
) {
  if (JSON.stringify(left.files) !== JSON.stringify(right.files)) {
    throw new Error(`${phase} changed canonical conversation/input storage`);
  }
  if (left.provider !== right.provider) {
    throw new Error(`${phase} changed provider request count`);
  }
}

export function assertFreshRawResponseIdentities(rawAttempts: string[]) {
  const pattern = /chatcmpl-[0-9a-f-]{36}/g;
  const identities = rawAttempts.map((raw) => raw.match(pattern) ?? []);
  const perResponse = identities.map((values) => {
    if (values.length === 0 || values.some((value) => value !== values[0])) {
      throw new Error("one HTTP response split completion identity");
    }
    return values[0];
  });
  if (new Set(perResponse).size !== perResponse.length) {
    throw new Error("distinct HTTP attempts collapsed completion identity");
  }
}

export function assertHandlerDeleteContract(
  ephemeral: boolean,
  ids: string[],
  after: Scan,
  deleteCalls: Set<string>,
) {
  if (!ephemeral) return;
  for (const id of ids) {
    if (!deleteCalls.has(id)) {
      throw new Error(`archived handler did not call deleteConversation: ${id}`);
    }
    if (after.records[id] || after.artifacts.includes(id)) {
      throw new Error(`capture adapter delete contract failed persisted scan: ${id}`);
    }
  }
}

export async function recordConversationAllocation(storage: string, id: string) {
  const directory = join(storage, "capture-adapter");
  const path = join(directory, "conversation-sequence.json");
  const allocated = Number(id.slice(id.lastIndexOf("-") + 1));
  let previous = 0;
  try {
    const value = JSON.parse(await readFile(path, "utf8"));
    previous = Number(value.conversation_sequence) || 0;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  if (!Number.isSafeInteger(allocated) || allocated <= previous) {
    throw new Error(`capture conversation sequence regression: ${id}`);
  }
  await mkdir(directory, { recursive: true });
  await writeFile(path, `${JSON.stringify({ conversation_sequence: allocated }, null, 2)}\n`);
}

export async function deletePersistedConversation(storage: string, id: string) {
  const files: Record<string, string> = {};
  await readTree(storage, storage, files);
  for (const [path, text] of Object.entries(files)) {
    const absolute = join(storage, path);
    if (path.split("/").some((part) => part === id || part.startsWith(`${id}.`))) {
      await rm(absolute, { force: true });
      continue;
    }
    if (path.endsWith(".json")) {
      await rewriteJson(absolute, text, id);
    } else if (path.endsWith(".jsonl")) {
      await rewriteJsonLines(absolute, text, id);
    }
  }
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
      output[path.slice(root.length + 1)] = await readFile(path, "utf8");
    }
  }
}

async function rewriteJson(path: string, text: string, id: string) {
  const mutation = removeArtifact(JSON.parse(text), id);
  if (mutation.remove) await rm(path, { force: true });
  else if (mutation.changed) {
    await writeFile(path, `${JSON.stringify(mutation.value, null, 2)}\n`);
  }
}

async function rewriteJsonLines(path: string, text: string, id: string) {
  const output: unknown[] = [];
  let changed = false;
  for (const line of text.split("\n").filter(Boolean)) {
    const mutation = removeArtifact(JSON.parse(line), id);
    changed ||= mutation.changed || mutation.remove;
    if (!mutation.remove) output.push(mutation.value);
  }
  if (!changed) return;
  const rendered = output.length > 0
    ? `${output.map((value) => JSON.stringify(value)).join("\n")}\n`
    : "";
  await writeFile(path, rendered);
}

function removeArtifact(value: unknown, id: string): Mutation {
  if (!value || typeof value !== "object") {
    return { value, changed: false, remove: false };
  }
  if (Array.isArray(value)) return removeArray(value, id);
  const object = value as Record<string, unknown>;
  if (object.id === id || object.conversation_id === id || object.conversationId === id) {
    return { value, changed: true, remove: true };
  }
  const output: Record<string, unknown> = {};
  let changed = false;
  for (const [name, child] of Object.entries(object)) {
    if (name === id) {
      changed = true;
      continue;
    }
    const mutation = removeArtifact(child, id);
    changed ||= mutation.changed || mutation.remove;
    if (!mutation.remove) output[name] = mutation.value;
  }
  return { value: changed ? output : value, changed, remove: false };
}

function removeArray(value: unknown[], id: string): Mutation {
  const output: unknown[] = [];
  let changed = false;
  for (const child of value) {
    const mutation = removeArtifact(child, id);
    changed ||= mutation.changed || mutation.remove;
    if (!mutation.remove) output.push(mutation.value);
  }
  return { value: changed ? output : value, changed, remove: false };
}

type DynamicKind = "CHAT_COMPLETION" | "STORED_RESPONSE" | "RESPONSE" |
  "MSG" | "FC" | "FCO" | "RS" | "UUID" | "CONVERSATION";
export type TimestampAttempt = { raw?: number; token: string; registered: boolean };
const MAX_UNIX_TIMESTAMP_SECONDS = 253_402_300_799;

export class Normalizer {
  maps = new Map<DynamicKind, Map<string, string>>();
  timestampTokens: string[] = [];
  token(kind: DynamicKind, original: string): string {
    let values = this.maps.get(kind);
    if (!values) { values = new Map(); this.maps.set(kind, values); }
    const present = values.get(original);
    if (present) return present;
    const token = `<${kind}_ID_${values.size + 1}>`;
    values.set(original, token);
    return token;
  }
  responseAttempt(): TimestampAttempt {
    return { token: `<TIMESTAMP_ID_${this.timestampTokens.length + 1}>`, registered: false };
  }
  response(value: unknown, attempt: TimestampAttempt): unknown {
    return this.value(value, attempt);
  }
  string(value: string): string {
    const uuid = "[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}";
    const patterns: Array<[DynamicKind, RegExp]> = [
      ["CHAT_COMPLETION", new RegExp(`^chatcmpl-${uuid}$`)],
      ["RESPONSE", new RegExp(`^resp_${uuid}$`)], ["MSG", new RegExp(`^msg_${uuid}$`)],
      ["FC", new RegExp(`^fc_${uuid}$`)], ["FCO", new RegExp(`^fco_${uuid}$`)],
      ["RS", new RegExp(`^rs_${uuid}$`)], ["UUID", new RegExp(`^${uuid}$`)],
      ["CONVERSATION", /^conv-fake-headless-[1-9][0-9]*$/],
    ];
    if (value.startsWith("resp_letta_")) return this.token("STORED_RESPONSE", value);
    for (const [kind, pattern] of patterns) if (pattern.test(value)) return this.token(kind, value);
    if (/^(chatcmpl-|resp_|msg_|fc_|fco_|rs_|conv-fake-headless-)/.test(value))
      throw new Error(`malformed dynamic identifier: ${value}`);
    return value;
  }
  value(value: unknown, attempt?: TimestampAttempt): unknown {
    if (typeof value === "string") return this.string(value);
    if (Array.isArray(value)) return value.map((child) => this.value(child, attempt));
    if (!value || typeof value !== "object") return value;
    const object = value as Record<string, unknown>;
    const timestampKey = responseTimestampKey(object);
    return Object.fromEntries(Object.entries(object).map(([name, child]) => [name,
      name === timestampKey ? this.timestamp(child, attempt) : this.value(child, attempt)]));
  }
  timestamp(value: unknown, attempt?: TimestampAttempt): string {
    if (!attempt) throw new Error("dynamic timestamp outside response attempt");
    if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0 ||
        value > MAX_UNIX_TIMESTAMP_SECONDS)
      throw new Error(`invalid Unix created timestamp: ${String(value)}`);
    if (attempt.raw !== undefined && attempt.raw !== value)
      throw new Error("created timestamp changed within response attempt");
    attempt.raw = value;
    if (!attempt.registered) {
      this.timestampTokens.push(attempt.token);
      attempt.registered = true;
    }
    return attempt.token;
  }
  relationships(): Record<string, string[]> {
    const output = Object.fromEntries([...this.maps].map(([kind, values]) => {
      const pairs = [...values].map(([raw, token]) => ({ raw, token }));
      assertIdentityBijection(pairs);
      return [kind.toLowerCase(), pairs.map((pair) => pair.token)];
    }));
    if (this.timestampTokens.length > 0) output.timestamp = [...this.timestampTokens];
    assertTimestampOrdinals(this.timestampTokens);
    return output;
  }
}

function responseTimestampKey(object: Record<string, unknown>): string | null {
  const kind = object.object;
  if (typeof kind === "string" && kind.startsWith("chat.completion")) return "created";
  return kind === "response" ? "created_at" : null;
}
function assertIdentityBijection(pairs: Array<{ raw: string; token: string }>) {
  for (const [index, left] of pairs.entries()) for (const right of pairs.slice(index + 1))
    if ((left.raw === right.raw) !== (left.token === right.token))
      throw new Error("raw/token identity collapse or split");
}
function assertTimestampOrdinals(tokens: string[]) {
  for (const [index, token] of tokens.entries())
    if (token !== `<TIMESTAMP_ID_${index + 1}>`)
      throw new Error(`noncontiguous timestamp attempt token: ${token}`);
}
function rejects(action: () => void) { try { action(); } catch { return true; } return false; }
export function assertTimestampMutationCoverage() {
  const shape = (raw: number[]) => raw.reduce((result, created) => {
    const attempt = result.normalizer.responseAttempt();
    result.values.push(result.normalizer.response({ object: "chat.completion", created }, attempt));
    return result;
  }, { normalizer: new Normalizer(), values: [] as unknown[] }).values;
  if (JSON.stringify(shape([1, 1])) !== JSON.stringify(shape([1, 2])))
    throw new Error("cross-attempt timestamp equality affected fixture shape");
  const normalizer = new Normalizer(); const attempt = normalizer.responseAttempt();
  normalizer.response({ object: "chat.completion.chunk", created: 1 }, attempt);
  if (!rejects(() => normalizer.response({ object: "chat.completion.chunk", created: 2 }, attempt)))
    throw new Error("within-attempt timestamp mutation escaped");
  for (const created of ["1", 1.5, -1, MAX_UNIX_TIMESTAMP_SECONDS + 1]) {
    const malformed = new Normalizer();
    if (!rejects(() => malformed.response(
      { object: "chat.completion", created }, malformed.responseAttempt())))
      throw new Error("malformed timestamp mutation escaped");
  }
  const staticValue = { object: "model", created: 1, sequence: 7, timestamp: 9 };
  if (JSON.stringify(new Normalizer().value(staticValue)) !== JSON.stringify(staticValue))
    throw new Error("static semantic timestamp was normalized");
}
