#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const PIN = "300f923f16cc8eee50656d7da732902c1dea2b65";
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = resolve(ROOT, "fixtures/protocol/discriminants.json");
const LIMITS = Object.freeze({
  sourceFiles: { value: 256, units: "source files" },
  declarations: { value: 4096, units: "declarations" },
  unionMembers: { value: 4096, units: "union members" },
  activeDepth: { value: 256, units: "active declarations" },
  workItems: { value: 16384, units: "work items" },
});

function fail(message) {
  throw new Error(`protocol fixture extraction failed: ${message}`);
}
function guard(limit, next) {
  if (next > limit.value) {
    fail(`resolution bound exceeded: ${limit.value} ${limit.units}`);
  }
}

class Resolver {
  constructor(source) {
    this.source = resolve(source);
    this.files = new Map();
    this.declarationCount = 0;
  }

  load(rel) {
    if (!this.files.has(rel)) {
      guard(LIMITS.sourceFiles, this.files.size + 1);
      const text = readFileSync(resolve(this.source, rel), "utf8");
      this.files.set(rel, { text });
    }
    return this.files.get(rel).text;
  }

  declarations(rel) {
    const file = this.files.get(rel);
    if (file?.declarations) return file.declarations;
    const text = this.load(rel);
    const map = new Map();
    const pattern = /export\s+(?:type|interface)\s+(\w+)\s*(?:=|extends\s+[^\{]+)?/g;
    for (const match of text.matchAll(pattern)) {
      guard(LIMITS.declarations, this.declarationCount + 1);
      this.declarationCount += 1;
      map.set(match[1], match.index);
    }
    this.files.set(rel, { text, declarations: map });
    return map;
  }

  modulePath(from, spec) {
    if (!spec.startsWith(".")) {
      fail(`cannot resolve external member import ${spec}`);
    }
    return `${resolve(dirname(resolve(this.source, from)), spec)
      .slice(this.source.length + 1)}.ts`;
  }

  imports(rel) {
    const file = this.files.get(rel);
    if (file?.imports) return file.imports;
    const text = this.load(rel);
    const named = new Map();
    const spaces = new Map();
    const namespacePattern =
      /import\s+type\s+\*\s+as\s+(\w+)\s+from\s+["']([^"']+)["']/g;
    for (const match of text.matchAll(namespacePattern)) {
      spaces.set(match[1], this.modulePath(rel, match[2]));
    }
    const namedPattern =
      /import\s+type\s*\{([\s\S]*?)\}\s+from\s+["']([^"']+)["']/g;
    for (const match of text.matchAll(namedPattern)) {
      if (!match[2].startsWith(".")) continue;
      const target = this.modulePath(rel, match[2]);
      const items = match[1].split(",").map((item) => item.trim());
      for (const item of items.filter(Boolean)) {
        const [original, alias] = item.split(/\s+as\s+/);
        named.set(alias ?? original, { rel: target, name: original });
      }
    }
    const imports = { named, spaces };
    this.files.set(rel, { text, declarations: this.declarations(rel), imports });
    return imports;
  }

  body(rel, name) {
    const text = this.load(rel);
    const start = this.declarations(rel).get(name);
    if (start === undefined) fail(`unresolved declaration ${name} in ${rel}`);
    const lineEnd = text.indexOf("\n", start);
    const line = text.slice(start, lineEnd < 0 ? text.length : lineEnd);
    if (line.includes("interface")) {
      const open = text.indexOf("{", start);
      let depth = 0;
      for (let index = open; index < text.length; index += 1) {
        if (text[index] === "{") depth += 1;
        if (text[index] === "}" && --depth === 0) {
          return text.slice(start, index + 1);
        }
      }
      fail(`unterminated declaration ${rel}#${name}`);
    }
    const semi = text.indexOf(";", start);
    if (semi < 0) fail(`unterminated declaration ${rel}#${name}`);
    return text.slice(start, semi + 1);
  }

  directLiteral(rel, name, body) {
    const matches = [];
    let depth = 0;
    for (const line of body.split("\n")) {
      const match = /^\s*type\s*:\s*["']([^"']+)["']/.exec(line);
      if (depth === 1 && match) matches.push(match[1]);
      depth += (line.match(/\{/g) ?? []).length;
      depth -= (line.match(/\}/g) ?? []).length;
    }
    if (matches.length > 1) fail(`multiple type literals in ${rel}#${name}`);
    return matches[0];
  }

  targets(rel, name) {
    const local = this.declarations(rel);
    if (!local.has(name)) {
      const imported = this.imports(rel).named.get(name);
      if (!imported) fail(`unresolved member ${name} in ${rel}`);
      return [{ ...imported, kind: "declaration" }];
    }
    const body = this.body(rel, name);
    const literal = this.directLiteral(rel, name, body);
    if (literal !== undefined) return [{ kind: "literal", value: literal }];
    const eq = body.indexOf("=");
    if (eq < 0) fail(`nonliteral type member ${rel}#${name}`);
    const expression = body.slice(eq + 1, -1);
    const union = expression.split("|").map((item) => item.trim()).filter(Boolean);
    guard(LIMITS.unionMembers, union.length);
    const refs = union.flatMap((member) =>
      member.split("&").map((item) => item.trim()).filter(Boolean));
    const targets = [];
    for (const ref of refs) {
      const qualified = /^(\w+)\.(\w+)$/.exec(ref);
      if (qualified) {
        const target = this.imports(rel).spaces.get(qualified[1]);
        if (!target) fail(`unresolved namespace ${qualified[1]} in ${rel}`);
        targets.push({ rel: target, name: qualified[2], kind: "declaration" });
      } else if (/^\w+$/.test(ref)) {
        const imported = this.imports(rel).named.get(ref);
        targets.push({ ...(imported ?? { rel, name: ref }), kind: "declaration" });
      }
    }
    if (targets.length === 0) {
      fail(`nonliteral or unresolved type member ${rel}#${name}`);
    }
    return targets;
  }

  expand(rel, name) {
    const output = [];
    const stack = [{ kind: "enter", rel, name }];
    const active = [];
    const activeIndex = new Map();
    let workItems = 1;
    while (stack.length > 0) {
      const item = stack.pop();
      if (item.kind === "leave") {
        activeIndex.delete(item.key);
        active.pop();
        continue;
      }
      if (item.kind === "literal") {
        output.push(item.value);
        continue;
      }
      const key = `${item.rel}#${item.name}`;
      const cycleAt = activeIndex.get(key);
      if (cycleAt !== undefined) {
        const path = active.slice(cycleAt).concat(key).join(" -> ");
        fail(`cyclic declaration ${path}`);
      }
      guard(LIMITS.activeDepth, active.length + 1);
      activeIndex.set(key, active.length);
      active.push(key);
      const targets = this.targets(item.rel, item.name);
      guard(LIMITS.workItems, workItems + targets.length + 1);
      workItems += targets.length + 1;
      stack.push({ kind: "leave", key });
      for (let index = targets.length - 1; index >= 0; index -= 1) {
        stack.push(targets[index].kind === "literal"
          ? targets[index]
          : { kind: "enter", ...targets[index] });
      }
    }
    return output;
  }
}

function unique(values, label) {
  const seen = new Set();
  for (const value of values) {
    if (seen.has(value)) fail(`duplicate ${label} discriminant ${value}`);
    seen.add(value);
  }
  return values;
}

function fixtureFor(source) {
  const resolver = new Resolver(source);
  const protocol = "src/types/protocol_v2.ts";
  const entrypoint = "src/types/app-server-protocol.ts";
  const exportPattern = /export\s+\*\s+from\s+["']\.\/protocol_v2["']/;
  if (!exportPattern.test(resolver.load(entrypoint))) {
    fail(`${entrypoint} does not publicly export protocol_v2.ts`);
  }
  return {
    schema_version: 1,
    source_commit: PIN,
    commands: {
      entrypoint: protocol,
      source: protocol,
      declaration: "WsProtocolCommand",
      discriminants: unique(
        resolver.expand(protocol, "WsProtocolCommand"), "command"),
    },
    messages: {
      entrypoint,
      source: protocol,
      declaration: "WsProtocolMessage",
      discriminants: unique(
        resolver.expand(protocol, "WsProtocolMessage"), "message"),
    },
  };
}

function expectFailure(label, files, start, expected) {
  const root = makeSource(files);
  try {
    new Resolver(root).expand("root.ts", start);
    fail(`self-test ${label} unexpectedly succeeded`);
  } catch (error) {
    if (!String(error.message).includes(expected)) {
      fail(`self-test ${label}: ${error.message}`);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}
function makeSource(files) {
  const root = mkdtempSync(resolve(tmpdir(), "lotta-extractor-"));
  for (const [name, text] of Object.entries(files)) {
    writeFileSync(resolve(root, name), text);
  }
  return root;
}
function chain(length) {
  const lines = [];
  for (let index = 0; index < length; index += 1) {
    const next = index + 1 === length
      ? 'export interface A' + index + ' {\n  type: "ok"\n}'
      : `export type A${index} = A${index + 1};`;
    lines.push(next);
  }
  return lines.join("\n");
}
function isCheckout(source) {
  try {
    execFileSync("git", ["-C", source, "rev-parse", "--is-inside-work-tree"], {
      stdio: "ignore",
    });
    return true;
  } catch {
    return false;
  }
}
function discoverSource(repoRoot, explicit, available = isCheckout) {
  if (explicit !== undefined) return resolve(explicit);
  const sibling = resolve(repoRoot, "..", "letta-code");
  const child = resolve(repoRoot, "letta-code");
  if (available(sibling)) return sibling;
  if (available(child)) return child;
  fail("source checkout not found; checked ../letta-code and "
    + "./letta-code; pass --source <checkout>");
}
function discoverySelfTest() {
  const root = mkdtempSync(resolve(tmpdir(), "lotta-discovery-"));
  const sibling = resolve(root, "..", "letta-code");
  const child = resolve(root, "letta-code");
  const explicit = resolve(root, "chosen");
  const available = (paths) => (candidate) => paths.has(candidate);
  mkdirSync(child);
  if (discoverSource(root, undefined, available(new Set([sibling, child]))) !== sibling) {
    fail("self-test discovery did not prefer sibling");
  }
  if (discoverSource(root, undefined, available(new Set([child]))) !== child) {
    fail("self-test discovery did not fall back to child");
  }
  try {
    discoverSource(root, undefined, available(new Set()));
    fail("self-test discovery without checkout unexpectedly succeeded");
  } catch (error) {
    if (!error.message.includes("../letta-code and ./letta-code")) throw error;
  }
  if (discoverSource(root, explicit, available(new Set([sibling, child]))) !== explicit) {
    fail("self-test explicit source did not win");
  }
  rmSync(root, { recursive: true, force: true });
}
function selfTest() {
  discoverySelfTest();
  const local = { "root.ts": "export type A = B;\nexport type B = A;" };
  expectFailure("local alias cycle", local, "A", "root.ts#A -> root.ts#B -> root.ts#A");
  const union = {
    "root.ts": "export type A = Leaf | A;\nexport interface Leaf {\n  type: \"x\"\n}",
  };
  expectFailure("local union cycle", union, "A", "root.ts#A -> root.ts#A");
  const named = {
    "root.ts": 'import type { B } from "./other";\nexport type A = B;',
    "other.ts": 'import type { A } from "./root";\nexport type B = A;',
  };
  expectFailure("named import cycle", named, "A", "root.ts#A -> other.ts#B -> root.ts#A");
  const space = {
    "root.ts": 'import type * as O from "./other";\nexport type A = O.B;',
    "other.ts": 'import type * as R from "./root";\nexport type B = R.A;',
  };
  expectFailure("namespace cycle", space, "A", "root.ts#A -> other.ts#B -> root.ts#A");
  for (const length of [LIMITS.activeDepth.value - 1, LIMITS.activeDepth.value]) {
    const root = makeSource({ "root.ts": chain(length) });
    const result = new Resolver(root).expand("root.ts", "A0");
    rmSync(root, { recursive: true, force: true });
    if (result.join() !== "ok") fail(`self-test chain ${length} failed`);
  }
  expectFailure(
    "over-depth chain",
    { "root.ts": chain(LIMITS.activeDepth.value + 1) },
    "A0",
    `${LIMITS.activeDepth.value} ${LIMITS.activeDepth.units}`,
  );
  expectFailure("unresolved", { "root.ts": "export type A = Missing;" }, "A", "unresolved");
  const duplicateRoot = makeSource({
    "root.ts": 'export interface A { type: "x" }',
  });
  try {
    unique(["x", "x"], "self-test");
  } catch (error) {
    if (!error.message.includes("duplicate")) throw error;
  } finally {
    rmSync(duplicateRoot, { recursive: true, force: true });
  }
  console.log("extractor self-test passed: discovery, cycle paths, and resolution bounds");
}

function main() {
  const args = process.argv.slice(2);
  if (args.includes("--self-test")) return selfTest();
  const sourceIndex = args.indexOf("--source");
  if (sourceIndex >= 0 && !args[sourceIndex + 1]) {
    fail("--source requires a checkout path");
  }
  const explicit = sourceIndex >= 0 ? args[sourceIndex + 1] : undefined;
  const source = discoverSource(ROOT, explicit);
  const head = execFileSync("git", ["-C", source, "rev-parse", "HEAD"], {
    encoding: "utf8",
  }).trim();
  if (head !== PIN) fail(`source HEAD ${head} does not match pinned ${PIN}`);
  const rendered = `${JSON.stringify(fixtureFor(source), null, 2)}\n`;
  if (args.includes("--check")) {
    const current = readFileSync(OUT, "utf8");
    if (current !== rendered) {
      const at = [...rendered].findIndex((char, index) => current[index] !== char);
      fail(`fixture differs at byte ${at}; regenerate with extractor --source ${source}`);
    }
  } else {
    writeFileSync(OUT, rendered, "utf8");
  }
}

try {
  main();
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
