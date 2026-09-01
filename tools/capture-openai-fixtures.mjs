import { createHash } from "node:crypto";
import {
  chmod,
  cp,
  mkdtemp,
  mkdir,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawn } from "node:child_process";
import process from "node:process";

const BASELINE_COMMIT = "300f923f16cc8eee50656d7da732902c1dea2b65";
const BASELINE_TREE = "30d2e2cebc7761c153f5a6d136b242365092faa0";
const CASES_MAX = 16;
const FIXTURE_BYTES_MAX = 1_048_576;
const COMMAND_TIMEOUT_MS = 180_000;
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
const PINNED = {
  handler: [
    "src/websocket/app-server-openai.ts",
    "22058763c116e87ea0d8decaa675c8c4df711e51f4895936832455a0376c2849",
  ],
  turn_bridge: [
    "src/websocket/app-server-openai-turn.ts",
    "08f445e1ed19304dff8227ae058e71f555ec69142b11e7c0073154e916eec531",
  ],
  common: [
    "src/websocket/app-server-openai-common.ts",
    "3053fcf764a9048ffafefba74d780c40a35ca1ed88df0131d6bc4f40d8482ccd",
  ],
  lockfile: [
    "bun.lock",
    "0a8cad33168b97cfd08958d29b853f683eff9a36f42d1709e761990a74adc26e",
  ],
};
const repository =
  process.env.LETTA_CODE_REPOSITORY ??
  "/Volumes/Delorean/code/five-letters/letta-code";
const output = resolve(
  process.argv[2] ?? new URL("../fixtures/openai", import.meta.url).pathname,
);
const runnerSource = new URL("./openai-capture-runner.ts", import.meta.url);
const adapterSource = new URL("./openai-capture-adapter.ts", import.meta.url);

function run(command, args, options = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, {
      ...options,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const stdout = [];
    const stderr = [];
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      const err = Buffer.concat(stderr).toString("utf8");
      reject(new Error(`command timed out: ${command} ${args.join(" ")}: ${err}`));
    }, COMMAND_TIMEOUT_MS);
    child.stdout.on("data", (chunk) => stdout.push(chunk));
    child.stderr?.on("data", (chunk) => stderr.push(chunk));
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      const out = Buffer.concat(stdout).toString("utf8");
      const err = Buffer.concat(stderr).toString("utf8");
      if (code === 0) resolvePromise(out);
      else reject(new Error(`${command} exited ${code}: ${err}`));
    });
  });
}
function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
async function verifyPinnedTree(tree) {
  for (const [label, [path, expected]] of Object.entries(PINNED)) {
    const actual = sha256(await readFile(join(tree, path)));
    if (actual !== expected)
      throw new Error(`${label} hash mismatch: ${actual}`);
  }
  const turn = await readFile(join(tree, PINNED.turn_bridge[0]), "utf8");
  if (
    !turn.includes("runTurnViaListenerRuntime") ||
    !turn.includes("dispatchInboundMessageWhenReady")
  ) {
    throw new Error("archived turn bridge authenticity markers missing");
  }
}
function validateCases(cases) {
  if (!Array.isArray(cases) || cases.length === 0 || cases.length > CASES_MAX)
    throw new Error("capture cases missing or over bound");
  const names = cases.map((fixture) => fixture.name);
  if (
    names.length !== REQUIRED_CASES.length ||
    REQUIRED_CASES.some((name) => !names.includes(name))
  )
    throw new Error("required baseline capture case missing");
  if (new Set(names).size !== names.length)
    throw new Error("duplicate capture case");
}
async function writeCorpus(cases, metadata) {
  await rm(output, { recursive: true, force: true });
  await mkdir(join(output, "cases"), { recursive: true });
  const entries = [];
  for (const fixture of cases) {
    if (!fixture.name || !fixture.route || !fixture.mode || !fixture.execution)
      throw new Error("captured case incomplete");
    const text = `${JSON.stringify(fixture, null, 2)}\n`;
    if (Buffer.byteLength(text) > FIXTURE_BYTES_MAX)
      throw new Error(`fixture too large: ${fixture.name}`);
    const path = `cases/${fixture.name}.json`;
    await writeFile(join(output, path), text);
    entries.push({
      name: fixture.name,
      route: fixture.route,
      mode: fixture.mode,
      dependencies: fixture.dependencies,
      path,
      sha256: sha256(text),
    });
  }
  const index = {
    schema_version: 2,
    baseline_commit: BASELINE_COMMIT,
    baseline_tree: BASELINE_TREE,
    capture_command: "LOTTA_BUN=bun node tools/capture-openai-fixtures.mjs",
    runtime: metadata.runtime,
    sources: metadata.sources,
    bounds: {
      cases_max: CASES_MAX,
      fixture_bytes_max: FIXTURE_BYTES_MAX,
      events_per_case_max: 64,
      json_depth_max: 32,
    },
    cases: entries,
  };
  await writeFile(
    join(output, "index.json"),
    `${JSON.stringify(index, null, 2)}\n`,
  );
  await chmod(join(output, "index.json"), 0o644);
}
async function archiveBaseline(temp) {
  const resolved = (
    await run("git", [
      "-C",
      repository,
      "rev-parse",
      `${BASELINE_COMMIT}^{commit}`,
    ])
  ).trim();
  if (resolved !== BASELINE_COMMIT)
    throw new Error(`baseline mismatch: ${resolved}`);
  const treeHash = (
    await run("git", [
      "-C",
      repository,
      "show",
      "-s",
      "--format=%T",
      BASELINE_COMMIT,
    ])
  ).trim();
  if (treeHash !== BASELINE_TREE)
    throw new Error(`baseline tree mismatch: ${treeHash}`);
  const archive = join(temp, "baseline.tar");
  await run("git", [
    "-C",
    repository,
    "archive",
    "--format=tar",
    "-o",
    archive,
    BASELINE_COMMIT,
  ]);
  const tree = join(temp, "tree");
  await mkdir(tree);
  await run("tar", ["-xf", archive, "-C", tree]);
  return tree;
}
async function main() {
  const temp = await mkdtemp(join(tmpdir(), "lotta-openai-capture-"));
  try {
    const tree = await archiveBaseline(temp);
    await verifyPinnedTree(tree);
    const bun = process.env.LOTTA_BUN ?? "/Users/stan/.bun/bin/bun";
    const runtime = (await run(bun, ["--version"])).trim();
    if (runtime !== "1.3.14")
      throw new Error(`Bun runtime mismatch: ${runtime}`);
    await run(bun, ["install", "--frozen-lockfile", "--ignore-scripts"], {
      cwd: tree,
      env: { ...process.env, HOME: join(temp, "home") },
    });
    await cp(runnerSource, join(tree, basename(runnerSource.pathname)));
    await cp(adapterSource, join(tree, basename(adapterSource.pathname)));
    const runnerHash = sha256(await readFile(runnerSource));
    const adapterHash = sha256(await readFile(adapterSource));
    const stdout = await run(bun, [basename(runnerSource.pathname)], {
      cwd: tree,
      env: {
        ...process.env,
        HOME: join(temp, "home"),
        LETTA_LOCAL_BACKEND_DIR: join(temp, "store"),
      },
    });
    const cases = JSON.parse(stdout.trim());
    validateCases(cases);
    const sources = Object.fromEntries(
      Object.entries(PINNED).map(([name, [path, hash]]) => [
        name,
        { path, sha256: hash },
      ]),
    );
    sources.capture_runner = {
      path: "tools/openai-capture-runner.ts",
      sha256: runnerHash,
    };
    sources.capture_adapter = {
      path: "tools/openai-capture-adapter.ts",
      sha256: adapterHash,
    };
    await writeCorpus(cases, {
      runtime: { name: "bun", version: runtime },
      sources,
    });
    console.log(`captured ${cases.length} cases from ${BASELINE_COMMIT}`);
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
}
await main();
