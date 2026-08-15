#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import {
  BOUNDARY,
  INVARIANTS,
  INVARIANT_PROVENANCE_SPECS,
  LIMITS,
  PIN,
  PLACEHOLDER,
  REGION_SPECS,
  REGIONS,
  RULES,
  SURFACES,
  WHOLE,
} from "./capture-reference-traces-data.mjs";
import {
  captureReferenceTraces,
} from "./capture-reference-traces-scenarios.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = resolve(ROOT, "fixtures/reference-traces");
function fail(message) {
  throw new Error(`reference trace capture failed: ${message}`);
}
function sha(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
function json(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}
function head(path) {
  return execFileSync("git", ["-C", path, "rev-parse", "HEAD"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  }).trim();
}
function isPinned(path) {
  try {
    return head(path) === PIN;
  } catch {
    return false;
  }
}
function discover(explicit) {
  if (explicit) {
    const path = resolve(explicit);
    if (!isPinned(path))
      fail("explicit source is not the pinned checkout");
    return path;
  }
  const candidates = [
    resolve(ROOT, "../letta-code"),
    resolve(ROOT, "../../letta-code"),
    resolve(ROOT, "letta-code"),
  ];
  for (const path of candidates) if (isPinned(path)) return path;
  fail(
    "pinned source not found in canonical sibling, workspace sibling, or CI child",
  );
}
function maskLexical(source) {
  let out = "",
    state = "code",
    quote = "",
    escaped = false;
  for (let index = 0; index < source.length; index += 1) {
    const c = source[index],
      next = source[index + 1];
    if (state === "line") {
      out += c === "\n" ? "\n" : " ";
      if (c === "\n") state = "code";
      continue;
    }
    if (state === "block") {
      out += c === "\n" ? "\n" : " ";
      if (c === "*" && next === "/") {
        out += " ";
        index += 1;
        state = "code";
      }
      continue;
    }
    if (state === "string") {
      out += c === "\n" ? "\n" : " ";
      if (!escaped && c === quote) state = "code";
      escaped = !escaped && c === "\\";
      continue;
    }
    if (c === "/" && next === "/") {
      out += "  ";
      index += 1;
      state = "line";
    } else if (c === "/" && next === "*") {
      out += "  ";
      index += 1;
      state = "block";
    } else if ("\"'`".includes(c)) {
      out += " ";
      state = "string";
      quote = c;
      escaped = false;
    } else out += c;
  }
  if (state === "block" || state === "string")
    fail("unterminated lexical source");
  return out;
}
function balancedRegion(source, masked, start, open) {
  let depth = 0;
  if (open < 0) fail("operative region has no body");
  for (let index = open; index < masked.length; index += 1) {
    if (masked[index] === "{") depth += 1;
    if (masked[index] === "}") depth -= 1;
    if (depth === 0) return source.slice(start, index + 1);
  }
  fail("operative region unbalanced");
}
function parseSimpleLiteral(source, index) {
  while (/\s/.test(source[index] ?? "")) index += 1;
  const quote = source[index];
  if (!"\"'`".includes(quote ?? "")) return undefined;
  let value = "";
  for (let cursor = index + 1; cursor < source.length; cursor += 1) {
    const c = source[cursor];
    if (c === quote) return { value, end: cursor + 1 };
    if (c === "\\") {
      const escaped = source[++cursor];
      if (escaped === undefined) fail("unterminated callback literal");
      const simple = { n: "\n", r: "\r", t: "\t" }[escaped];
      value += simple ?? escaped;
    } else {
      if (quote === "`" && c === "$" && source[cursor + 1] === "{")
        fail("interpolated callback literal");
      if (c === "\n" || c === "\r") fail("unterminated callback literal");
      value += c;
    }
  }
  fail("unterminated callback literal");
}
function namedCallbackRegion(source, masked, symbol) {
  const [kind, name] = symbol.split(":", 2),
    pattern = new RegExp(`\\b${kind}\\s*\\(`, "g"),
    hits = [];
  for (const match of masked.matchAll(pattern)) {
    const literal = parseSimpleLiteral(source, match.index + match[0].length);
    if (literal?.value === name) hits.push({ start: match.index, end: literal.end });
  }
  if (hits.length !== 1) fail(`operative callback count: ${symbol}`);
  const { start, end } = hits[0],
    arrow = masked.indexOf("=>", end);
  if (arrow < 0) fail("operative callback has no arrow");
  return balancedRegion(source, masked, start, masked.indexOf("{", arrow));
}
function classMethodRegion(source, masked, name) {
  const classStart = masked.indexOf("export class AppServerClient");
  const pattern = new RegExp(
    `(?:private\\s+)?(?:async\\s+)?${name}\\s*(?:<[^>{}]*>)?\\s*\\(`,
    "g",
  );
  const hits = [...masked.slice(classStart).matchAll(pattern)];
  const definitions = hits.filter((hit) => {
    const before = masked[classStart + hit.index - 1] ?? "\n";
    return before === "\n" || before === "{";
  });
  if (definitions.length > 1 || !hits.length)
    fail(`operative method count: ${name}`);
  const hit = definitions[0] ?? hits[hits.length - 1],
    start = classStart + hit.index;
  return balancedRegion(
    source,
    masked,
    start,
    masked.indexOf("{", start),
  );
}
function operativeRegion(source, symbol) {
  const masked = maskLexical(source);
  if (symbol.startsWith("test:") || symbol.startsWith("describe:")) {
    return namedCallbackRegion(source, masked, symbol);
  }
  if (symbol.startsWith("method:"))
    return classMethodRegion(source, masked, symbol.slice(7));
  const pattern = new RegExp(
    `(?:export\\s+)?(?:async\\s+)?` +
      `(?:function|class|interface|type|const)\\s+${symbol}\\b`,
    "g",
  );
  const hits = [...masked.matchAll(pattern)];
  if (hits.length !== 1) fail(`operative symbol count: ${symbol}`);
  const start = hits[0].index;
  return balancedRegion(
    source,
    masked,
    start,
    masked.indexOf("{", start),
  );
}
function verifySources(root) {
  const before = head(root);
  if (before !== PIN) fail("source moved before read");
  for (const [path, expected] of Object.entries(WHOLE)) {
    if (sha(readFileSync(resolve(root, path))) !== expected)
      fail(`whole drift: ${path}`);
  }
  const expected = new Map(
    REGIONS.map(([path, symbol, hash]) => [`${path}:${symbol}`, hash]),
  );
  const regions = REGION_SPECS.map(([path, symbol]) => {
    const hash = sha(
      operativeRegion(
        readFileSync(resolve(root, path), "utf8"),
        symbol,
      ),
    );
    const pinned = expected.get(`${path}:${symbol}`);
    if (pinned && hash !== pinned)
      fail(`operative drift: ${path}:${symbol}`);
    return { path, symbol, sha256: pinned ?? hash };
  });
  if (head(root) !== before) fail("source moved during read");
  return regions;
}

async function buildCorpus(sourceRoot, regions) {
  const traces = await captureReferenceTraces(sourceRoot);
  const regionKeys = new Set(
    regions.map((x) => `${x.path}:${x.symbol}`),
  );
  for (const value of traces) {
    for (const ref of [
      value.provenance,
      ...value.supporting_provenance,
    ]) {
      if (!regionKeys.has(`${ref.path}:${ref.symbol}`))
        fail(`case provenance does not resolve: ${value.name}`);
    }
  }
  const files = new Map();
  for (const value of traces)
    put(files, `${value.name}.json`, json(value));
  const inventory = [...files]
    .map(([path, bytes]) => ({
      path,
      kind: "json",
      bytes: bytes.length,
      sha256: sha(bytes),
    }))
    .sort((a, b) => a.path.localeCompare(b.path));
  const sourceFiles = Object.entries(WHOLE).map(([path, sha256]) => ({
    path,
    sha256,
  }));
  const cases = traces.map((value) => ({
    name: value.name,
    path: `${value.name}.json`,
    kind: value.kind,
    provenance: value.provenance,
    supporting_provenance: value.supporting_provenance,
    driver_proof: value.driver_proof,
  }));
  const invariantRegions = INVARIANT_PROVENANCE_SPECS.map(
    ([invariant, path, symbol]) => ({
      invariant,
      path,
      symbol,
    }),
  );
  if (
    invariantRegions.some(
      (x) => !regionKeys.has(`${x.path}:${x.symbol}`),
    )
  )
    fail("invariant provenance resolution");
  const index = {
    schema_version: 1,
    source_commit: PIN,
    generator: "tools/capture-reference-traces.mjs",
    capture_boundary: BOUNDARY,
    boundary_detail:
      "Observed deterministic real pinned AppServerClient sessions using an " +
      "in-process ReferenceSocket; no live provider, server process, network, " +
      "or package install.",
    source_files: sourceFiles,
    source_regions: regions,
    reliability_surfaces: SURFACES,
    ordering_invariants: INVARIANTS,
    invariant_provenance: invariantRegions,
    semantic_rules: RULES,
    cases,
    inventory,
  };
  return { files, index };
}
function put(files, path, text) {
  const bytes = Buffer.from(text);
  let total = bytes.length;
  for (const value of files.values()) total += value.length;
  if (
    files.size >= LIMITS.files - 1 ||
    files.has(path) ||
    bytes.length > LIMITS.fileBytes ||
    total > LIMITS.totalBytes ||
    path.length > LIMITS.nameBytes
  )
    fail("output bound");
  sanitize(path, bytes);
  files.set(path, bytes);
}
function decodedText(bytes) {
  const text = bytes.toString("utf8");
  if (!Buffer.from(text).equals(bytes)) fail("non-UTF8 fixture");
  return text
    .replace(/\\u([0-9a-f]{4})/gi, (_, x) =>
      String.fromCharCode(parseInt(x, 16)),
    )
    .replace(/\\[nrt]/g, " ");
}
function forbiddenText(value) {
  const lower = value.toLowerCase(),
    compact = lower.replace(/[^a-z0-9]/g, "");
  return (
    lower.includes("bearer ") ||
    lower.includes("private key") ||
    lower.includes("-----begin") ||
    [
      "apikey",
      "password",
      "accesstoken",
      "refreshtoken",
      "clientsecret",
      "oauth",
      "secretkey",
      "awsaccesskey",
      "googleapplicationcredentials",
    ].some((x) => compact.includes(x)) ||
    /eyJ[A-Za-z0-9_-]*\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/.test(value)
  );
}
function payloadKey(key, parent) {
  const compact = key.toLowerCase().replace(/[^a-z0-9]/g, "");
  return (
    [
      "content",
      "toolargs",
      "toolinput",
      "tooloutput",
      "prompt",
      "input",
      "output",
    ].includes(compact) ||
    (key === "delta" && parent === "message")
  );
}
function scanValue(value, parent = undefined) {
  if (Array.isArray(value)) {
    for (const child of value) scanValue(child, parent);
    return;
  }
  if (value && typeof value === "object") {
    for (const [key, child] of Object.entries(value)) {
      if (forbiddenText(key)) fail("secret-like field");
      if (
        payloadKey(key, parent) &&
        typeof child === "string" &&
        child !== PLACEHOLDER
      ) {
        fail(`retained payload: ${key}`);
      }
      scanValue(child, key);
    }
  } else if (typeof value === "string" && forbiddenText(value))
    fail("secret-like value");
}
function sanitize(path, bytes) {
  const text = decodedText(bytes);
  if (forbiddenText(text)) fail(`unsanitized fixture: ${path}`);
  rejectDuplicateKeys(text, path);
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    fail(`malformed JSON: ${path}`);
  }
  scanValue(value);
  if (path !== "index.json" && !text.includes(PLACEHOLDER))
    fail(`missing placeholder: ${path}`);
}

function duplicateKeyParser(text, path) {
  let index = 0;
  const whitespace = () => {
    while (/\s/.test(text[index] ?? "")) index += 1;
  };
  const string = () => {
    const parsed = parseDuplicateKeyString(text, path, index);
    index = parsed.index;
    return parsed.value;
  };
  const value = () => {
    whitespace();
    const token = text[index];
    if (token === "{") object();
    else if (token === "[") array();
    else if (token === '"') string();
    else {
      const match = text
        .slice(index)
        .match(
          new RegExp(
            "^(?:true|false|null|-?(?:0|[1-9]\\d*)" +
              "(?:\\.\\d+)?(?:[eE][+-]?\\d+)?)",
          ),
        );
      if (!match) fail(`malformed JSON value: ${path}`);
      index += match[0].length;
    }
    whitespace();
  };
  const object = () => {
    index += 1;
    whitespace();
    const keys = new Set();
    if (text[index] === "}") {
      index += 1;
      return;
    }
    while (index < text.length) {
      if (text[index] !== '"') fail(`malformed JSON object: ${path}`);
      const key = string();
      whitespace();
      if (keys.has(key)) fail(`duplicate JSON key: ${path}:${key}`);
      keys.add(key);
      if (text[index++] !== ":") fail(`malformed JSON object: ${path}`);
      value();
      if (text[index] === "}") {
        index += 1;
        return;
      }
      if (text[index++] !== ",") fail(`malformed JSON object: ${path}`);
      whitespace();
    }
    fail(`unterminated JSON object: ${path}`);
  };
  const array = () => parseDuplicateKeyArray(parser);
  const parser = {
    text,
    path,
    whitespace,
    value,
    getIndex: () => index,
    setIndex: (next) => {
      index = next;
    },
  };
  return parser;
}
function parseDuplicateKeyString(text, path, index) {
  const start = index;
  index += 1;
  while (index < text.length) {
    if (text[index] === "\\") {
      index += 2;
      continue;
    }
    if (text[index] === '"') {
      index += 1;
      try {
        return { value: JSON.parse(text.slice(start, index)), index };
      } catch {
        fail(`malformed JSON string: ${path}`);
      }
    }
    if (text.charCodeAt(index) < 0x20)
      fail(`malformed JSON string: ${path}`);
    index += 1;
  }
  fail(`unterminated JSON string: ${path}`);
}
function parseDuplicateKeyArray(parser) {
  const { path, text, value, whitespace } = parser;
  let index = parser.getIndex() + 1;
  parser.setIndex(index);
  whitespace();
  if (text[parser.getIndex()] === "]") {
    parser.setIndex(parser.getIndex() + 1);
    return;
  }
  while (parser.getIndex() < text.length) {
    value();
    index = parser.getIndex();
    if (text[index] === "]") {
      parser.setIndex(index + 1);
      return;
    }
    if (text[index] !== ",") fail(`malformed JSON array: ${path}`);
    parser.setIndex(index + 1);
    whitespace();
  }
  fail(`unterminated JSON array: ${path}`);
}
function rejectDuplicateKeys(text, path) {
  const parser = duplicateKeyParser(text, path);
  parser.value();
  if (parser.getIndex() !== text.length)
    fail(`trailing JSON data: ${path}`);
}

function scanTree(root) {
  const files = new Map(),
    stack = [[root, 0]];
  let total = 0;
  while (stack.length) {
    const [directory, depth] = stack.pop();
    if (depth > LIMITS.depth) fail("tree depth");
    const names = readdirSync(directory);
    if (depth && !names.length) fail("empty directory");
    for (const name of names) {
      if (!name || Buffer.byteLength(name) > LIMITS.nameBytes)
        fail("tree name bound");
      const path = resolve(directory, name),
        stat = lstatSync(path);
      if (
        stat.isSymbolicLink() ||
        (!stat.isFile() && !stat.isDirectory())
      )
        fail("special tree entry");
      if (stat.isDirectory()) stack.push([path, depth + 1]);
      else {
        if (files.size >= LIMITS.files || stat.size > LIMITS.fileBytes)
          fail("tree file bound");
        total += stat.size;
        if (total > LIMITS.totalBytes) fail("tree aggregate bound");
        files.set(
          relative(root, path).replaceAll(sep, "/"),
          readFileSync(path),
        );
      }
    }
  }
  return files;
}
function expectedTree(corpus) {
  const expected = new Map(corpus.files);
  expected.set("index.json", Buffer.from(json(corpus.index)));
  return expected;
}
function checkTree(corpus) {
  const actual = scanTree(OUT),
    expected = expectedTree(corpus);
  if (actual.size !== expected.size) fail("tree count mismatch");
  for (const [path, bytes] of expected) {
    if (!actual.has(path) || !actual.get(path).equals(bytes))
      fail(`fixture mismatch: ${path}`);
    sanitize(path, bytes);
  }
  for (const path of actual.keys())
    if (!expected.has(path)) fail(`extra fixture: ${path}`);
}
function confine(root, target) {
  if (target !== root && !target.startsWith(`${root}${sep}`))
    fail("path escape");
}
function writeCorpus(corpus) {
  const parent = dirname(OUT),
    temp = mkdtempSync(resolve(parent, ".reference-traces-"));
  try {
    for (const [path, bytes] of expectedTree(corpus)) {
      const target = resolve(temp, path);
      confine(temp, target);
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, bytes, { mode: 0o644 });
    }
    scanTree(temp);
    const backup = resolve(
      parent,
      `.reference-traces-backup-${process.pid}`,
    );
    rmSync(backup, { recursive: true, force: true });
    let moved = false;
    try {
      renameSync(OUT, backup);
      moved = true;
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    try {
      renameSync(temp, OUT);
    } catch (error) {
      if (moved) renameSync(backup, OUT);
      throw error;
    }
    rmSync(backup, { recursive: true, force: true });
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}
function expectRegionReject(source, symbol) {
  let failed = false;
  try {
    operativeRegion(source, symbol);
  } catch {
    failed = true;
  }
  if (!failed) fail(`region rejection self-test: ${symbol}`);
}
function regionHashSelfTest() {
  const samples = [
    [
      "function exact() { return 1; } // function exact() {}\n" +
        "const s = 'function exact() {}'; const t = `function exact() {}`;",
      "exact",
      "d1471ca467460bfe6aadbf30f30840f25af53d79713423e517b67af5748efdde",
      "return 1",
      "return 2",
    ],
    [
      "export class AppServerClient {\n  exact() { return 1; }\n} // exact() {}\n" +
        "const s = 'exact() {}'; const t = `exact() {}`;",
      "method:exact",
      "b2f4539f6d64ec5e4b9103ec3a4bdeaca500e7296a7b200a3ec36c1599c17c32",
      "return 1",
      "return 2",
    ],
    [
      "test(\"exact\", () => { return 1; }); // test(\"exact\", () => {})\n" +
        "const s = 'test(\"exact\", () => {})'; " +
        "const t = `test(\"exact\", () => {})`;",
      "test:exact",
      "73a808e6defc9c99aa5b44ededfc124594441ecdd05a71df334be02e232c4cbb",
      "return 1",
      "return 2",
    ],
    [
      "describe(`exact`, () => { return 1; }); // describe(`exact`, () => {})\n" +
        "const s = 'describe(`exact`, () => {})'; const t = `describe decoy`;",
      "describe:exact",
      "2c7d0177f69485c4b791a8894f3c556b581dbbe15cecd61d953f2c3d072c1fd6",
      "return 1",
      "return 2",
    ],
  ];
  for (const [source, symbol, expected, before, after] of samples) {
    if (sha(operativeRegion(source, symbol)) !== expected)
      fail(`region hash baseline self-test: ${symbol}`);
    const mutated = source.replace(before, after);
    if (sha(operativeRegion(mutated, symbol)) === expected)
      fail(`region hash mutation self-test: ${symbol}`);
  }
}
function selfTest() {
  regionHashSelfTest();
  const whole = "whole sample\n";
  if (sha(whole) !== "fd960e1eb4c298f0f7b1ed3c31cbef0e666f8271e49cc9383337fcedf3635bd0")
    fail("whole hash baseline self-test");
  if (sha("whole samplf\n") === sha(whole)) fail("whole hash mutation self-test");
  const rejects = [
    ["function exact(){} function exact(){}", "exact"],
    ["function exact(){", "exact"],
    ["export class AppServerClient {\nexact(){}\nexact(){}\n}", "method:exact"],
    ["export class AppServerClient { exact(){", "method:exact"],
    ["test('exact',()=>{}); test('exact',()=>{});", "test:exact"],
    ["describe('exact',()=>{}); describe('exact',()=>{});", "describe:exact"],
    ["test('exact',()=>{", "test:exact"],
    ["describe('exact',()=>{", "describe:exact"],
    ["test('exact,()=>{});", "test:exact"],
    ["describe(`exact ${value}`,()=>{});", "describe:exact"],
  ];
  for (const [source, symbol] of rejects) expectRegionReject(source, symbol);
  for (const value of [
    '{"a":1,"a":2}',
    '{"a":1,"\u0061":2}',
    '{"x":{"a":1,"a":2}}',
    '{"a":1',
    "[1,",
    '{"a":"unterminated}',
  ]) {
    let failed = false;
    try {
      rejectDuplicateKeys(value, "mutation.json");
    } catch {
      failed = true;
    }
    if (!failed) fail("duplicate-key mutation self-test");
  }
  rejectDuplicateKeys(
    '{"a":1,"nested":{"a":2},"items":[{"a":3}]}',
    "allowed.json",
  );
  for (const value of [
    '{"password":"x"}',
    '{"x":"Bearer x"}',
    '{"x":"eyJabc.def.ghi"}',
  ]) {
    let failed = false;
    try {
      sanitize("mutation.json", Buffer.from(value));
    } catch {
      failed = true;
    }
    if (!failed) fail("sanitizer mutation self-test");
  }
  const temp = mkdtempSync(resolve(tmpdir(), "trace-tree-test-"));
  try {
    writeFileSync(
      resolve(temp, "x"),
      Buffer.alloc(LIMITS.fileBytes + 1),
    );
    let failed = false;
    try {
      scanTree(temp);
    } catch {
      failed = true;
    }
    if (!failed) fail("tree bound self-test");
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
}
function parseArgs(argv) {
  const result = { check: false, selfTest: false, source: undefined };
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--check") result.check = true;
    else if (argv[index] === "--self-test") result.selfTest = true;
    else if (argv[index] === "--source" && argv[index + 1])
      result.source = argv[++index];
    else fail(`unknown argument: ${argv[index]}`);
  }
  return result;
}
async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.selfTest) {
    selfTest();
    return;
  }
  const source = discover(options.source),
    regions = verifySources(source);
  const corpus = await buildCorpus(source, regions);
  if (head(source) !== PIN) fail("source moved after capture");
  if (options.check) checkTree(corpus);
  else writeCorpus(corpus);
}
await main();
