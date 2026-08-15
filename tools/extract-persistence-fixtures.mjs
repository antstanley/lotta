#!/usr/bin/env node
/**
 * Dependency-free source-derived persistence fixture harness.
 *
 * The pinned checkout intentionally needs no installed runtime packages: this script asserts
 * exact implementation evidence, then reproduces its persisted layouts and wire formats.
 */
import { execFileSync } from "node:child_process";

import { createHash } from "node:crypto";

import {
  lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync,
} from "node:fs";

import { tmpdir } from "node:os";

import { dirname, relative, resolve, sep } from "node:path";

import { fileURLToPath } from "node:url";

const PIN = "300f923f16cc8eee50656d7da732902c1dea2b65";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const OUT = resolve(ROOT, "fixtures/persistence");

const AGENT = "agent-local-fixture";

const CONVERSATION = "conversation-fixture";

const TIME = "2000-01-01T00:00:00.000Z";

const PLACEHOLDER = "<redacted-fixture>";

const LIMITS = Object.freeze({ files: 256, bytes: 1048576, depth: 16, diagnostics: 12, });

const CASE_NAMES = Object.freeze([
  "current_typescript_state", "rust_target_state", "unversioned_legacy_transcript",
  "versioned_legacy_transcript", "baseline_tolerated_versioned_rows", "orphan_result_repair_input",
  "interrupted_append", "interrupted_replacement", "corrupt_unsupported_manifests", ]);

const SIDE_PATHS = Object.freeze([
  "side_stores/home/.letta/settings.json", "side_stores/letta_home/crons.json",
  "side_stores/letta_home/runs/schedule-fixture.jsonl", "side_stores/providers/auth.json",
  "side_stores/home/.letta/channels/pending-control-requests.json",
  "side_stores/home/.letta/channels/telegram/config.yaml",
  "side_stores/home/.letta/channels/telegram/accounts.json",
  "side_stores/home/.letta/channels/telegram/routing.yaml",
  "side_stores/home/.letta/channels/telegram/pairing.yaml",
  "side_stores/home/.letta/channels/telegram/targets.json",
  "side_stores/workspace/.letta/settings.json", "side_stores/workspace/.letta/settings.local.json",
]);

const SOURCES = Object.freeze({ "src/backend/local/local-store.ts":
    "b36e7b369a36519a1edbe320150f7da25bad3badaf39bd04ce81ac8f1c837f17",
  "src/backend/local/transcript-migration.ts":
    "95c451b91f8e0fa6c0d16e886b600225329e8b4331a5d0c9ab768619cda2a3cf",
  "src/backend/local/local-message-projection.ts":
    "ef54e281aa6ca8fd24e00ca910a9d65c790757d98503ef61973e28340492464a",
  "src/backend/local/local-provider-auth-store.ts":
    "e36529a2d5e877f958063ca9e25ad973013be27329fcf70de2e04a191d39a542",
  "src/backend/local/paths.ts": "e40ba6fa3fcab7f1cade10c306fe6f91d4b5d5a4e93fb35e789c82a1d779c1a2",
  "src/settings.ts": "b70d15dca6216bb24975bba041832fb390ee66604f4192aef1ebd9ba44b19f9f",
  "src/settings-manager.ts": "1c736bf5901fecd40da0bcbc2c378dc9794a5124b47c3ef2386394d485c2495d",
  "src/cron/cron-file.ts": "af6510fe3e38664d1cb1fc5e81c6fe8c90e6fbbb54f729af11b6a9b4f7095023",
  "src/cron/run-log.ts": "6111d8f91264f6f4b0c17b9d3ae2c33944cbf81a188694ae04cd24507d3a30d8",
  "src/channels/config.ts": "33df6a47d34bee4c9cc642d1e58e78d38633ed3e29d35dae5e7dd0411daaa6b6",
  "src/channels/accounts.ts": "09f82b48c3d6975651fe7447f593ebd23abbbe461b96c238e75dc069096e2be2",
  "src/channels/routing.ts": "fd43563c36e9a5286d802257c103cc82a2f851c0de57a7ec092297a7055af402",
  "src/channels/pairing.ts": "edaba564f316bf6f896166938dca0c3905188a4d04714807591922a5d4a78969",
  "src/channels/targets.ts": "3e0b4ea1111f014f31b75fbad9c4c9f22a360f17000618ac4b3724dd526d7f55",
  "src/channels/types.ts": "384671f6b38e78c182cf7bf48c8553f5983880a080b9f3b9dfcee3957efb1b9c",
  "src/channels/pending-control-requests.ts":
    "08195bfe4389774029f74a62d85205347b8296423579dafa435956ef416fbe38", });

const OPERATIVE = Object.freeze({ "src/backend/local/local-store.ts": [
    ["encodePathSegment", "29f568dbb55b18a3c12cc74676bb69e2c2c7b148e91f81bec6cc2cc4e3363619"],
    ["readJsonlFile", "573c30b42ff6790a06863727d377a1d205c1199a370e78d6588bdc3482833a67"], [
      "LOCAL_TRANSCRIPT_LEGACY_SCHEMA_VERSION",
      "31762f3eef06bec0c4a20424c8ef0df4528e39efdad5c129f8afe798e0e1ae31", ], [
      "LOCAL_TRANSCRIPT_SCHEMA_VERSION",
      "bb4d99ee63b69f5de0f10f26550e8886b50b6e07b41518b7f8968400cab55b4f", ], [
      "LOCAL_TRANSCRIPT_LEGACY_MESSAGE_FORMAT",
      "4cafac8bf784c3547287f3de313d4addc021de1442f98ce1406ed1b597a9533d", ], [
      "LOCAL_TRANSCRIPT_MESSAGE_FORMAT",
      "f1a5bdaa436c269f8d69fafd91cce6412d9a9d16151d7132524849a672714eeb", ], [
      "LOCAL_TRANSCRIPT_PROVIDER_STACK",
      "08f5b3fe87f120737f1281180f300e6bcf7bbf02d208e79121f980395f63e90b", ],
    ["LocalTranscriptManifest", "5bf5a954fad2cfe6eb882e0a227f6817c156bc6431135c87e35357ee5f45c546"],
    [ "LocalTranscriptSessionHeader",
      "777b6dce876a6b9fd6a3cf701f75cb4a777bfa00206fb6f1534359a9818e755a", ], [
      "LocalTranscriptSessionMessageEntry",
      "c49abbaa4c183b843ddc4e22ebb43907ec3ff8af326098dacc44a38e1a05949f", ], [
      "LocalTranscriptCompactionEntry",
      "d3849c966d0e94b65d8c91c9d4a959d799b7b247d48e4bb13abb55eac9332666", ], [
      "isLocalTranscriptSessionMessageEntry",
      "e2c69ce7ce49d4a839ea1fa2218201bfe7e63a8adcb72f8858b2e8f706ba8087", ], [
      "isLocalTranscriptCompactionEntry",
      "5beb9070d8364d0728a79722cf0d1295b928aaccefedf0826d7c744403bd7019", ], [
      "localTranscriptRowsResult",
      "c2cbe98a1d4fd72872a765ba3c0017f5eb76da3b65c53ddac4ce645071372b16", ], [
      "createLocalTranscriptManifest",
      "f5cf9f45a43780111fcaa6be264ba4ac38e6d36d25f04e3defbc2d2c6db2d4b4", ], [
      "validateLocalTranscriptManifest",
      "7cc8bf4a829aaa8a8e09353e939829f64c7ad124a16711beed7985f2887e22e6", ],
    ["conversationDirForKey", "c7ac99018cb52cb8bf86cab59668796562f090170665a6a757ff1ebb5c6af572"],
  ], "src/backend/local/transcript-migration.ts": [
    ["isLegacyUiMessage", "2aeb74ab546d0845a509fa981cfd2a281c54f44ca1a469388fac1bc3883e4da7"],
    ["isPiLocalMessage", "c1552c3c698b872f5b1c0d03465e749ad0dd6505a3c5a2d01948ffa2ad05e6a4"],
    ["writeSessionEntryJsonl", "0cd1c7df5608d085e4cf25d07cd36eb2e6455761051fca5bb085903f19398065"],
    ["manifest", "4f8b2b432dec2e21d86d145a88d4796acd68a80b151f770c400dc30b99406c62"],
    ["convertMessages", "a63ba068a28e31500eff52cbef51f7fff63658460f48903d41726f0f845319b7"], [
      "migrateLocalBackendTranscripts",
      "67e90687328468a664df280129b9bfa6132a39fc541c835ea01d17527e10446f", ], ],
  "src/backend/local/local-message-projection.ts": [ [ "removeOrphanLocalToolResults",
      "3c9bf3065a6c8aee773c0769f37d026276da85c96e3fee2ee8339b33149aa266", ], ],
  "src/backend/local/local-provider-auth-store.ts": [
    ["LocalProviderApiAuth", "971e0fc19de0339e09c25ced4784a34f2d4628514937a720061fd00edd398c6e"],
    ["LocalProviderRecord", "54f73f665a7d8a33619670cc6d6984c4ad6579ee891c32ee1d1773b8e0762eb9"],
    ["LocalProviderAuthFile", "7c3884decbb2ef6ccaa0e683e13aad5893c4528d0f49c44d7dd49aacb7e503d5"], [
      "getLocalProviderAuthPath",
      "14197231646082f3fd2b09b30f8f43b3bbd1ec1e9a22ce427a0352606ef7745d", ],
    ["emptyAuthFile", "7c23401e3e86bc87c70b50a2ab353a1feafe46161b0b20dcb98594189bbc19fe"],
    ["readAuthFile", "a7d6f958d31e74865001fbf9ae1dae96569967a39120d01b45a2109e58730f0f"],
    ["writeAuthFile", "02d9df0cefdd92192f1fe3fba237054ab06d5f985956cbd0b9030657e457c03e"], ],
  "src/backend/local/paths.ts": [ [ "getLocalBackendMemoryFilesystemRoot",
      "081659b5de5ca81d1a2e3d6206c19b8b9f5690a2194ac60f9f3dd45f284d7e73", ], ], "src/settings.ts": [
    ["Settings", "7bbd13f56ec316504275581dfd7b0cf130242ddc6db77282b4a4eff0ffe61c9f"],
    ["ProjectSettings", "312369630ff3a5fb5e88bc75aac9ef8b268de119d53384cf3a81872b2c21a435"],
    ["DEFAULT_SETTINGS", "8e9d020a4f90745b4882bee8614c2b4280677830e375305b493f71d6b090c206"],
    ["getSettingsPath", "edbb4458ac3b1d2db9d4c31169b6968c152ac3b75a0fcd17bba06b3789b3c5de"],
    ["getProjectSettingsPath", "567b778fd0270b75678822afc6e34cdf18cd5c1dc0c984ced3558a0877295875"],
  ], "src/settings-manager.ts": [
    ["Settings", "6c52fd75a0f9f90fc94c0cf451a77189d19423865f01a8c0f56e3b690e38bc2a"],
    ["ProjectSettings", "b35b9c230aea76dcb7b4b576e0bc05f6ee9b89146208efac548a6ad51df13ac9"],
    ["LocalProjectSettings", "e214cc5d21200a0acd5471d95be6347f09be0db1d9b04524c3e2929e77ca5fd9"],
    ["DEFAULT_SETTINGS", "dd940ca71971b900a85d809b6b007830df1840fda2c76cbcc264b152d9f4f670"], [
      "DEFAULT_PROJECT_SETTINGS",
      "9253b0fa95b5649085e11865b3d8a308a68e2edea13d65ee25cd93e2706e7b8b", ], [
      "DEFAULT_LOCAL_PROJECT_SETTINGS",
      "3cd3429b37dc528ee718f04fcfbad57cc830d65a8315e42a95beba96111b998c", ], ],
  "src/cron/cron-file.ts": [
    ["CronTask", "801b0781fd7afa1b89f305645ec6ecf770e6d1142c438708131da17329997378"],
    ["CronFileData", "80e1e6e339c353d1b783e1c1140cad4484fe1ee9f7745a5216db259918a10d9b"],
    ["getCronFilePath", "50680a788d1b03142673380732a8ba19024da4e34021b237e33588953f2862e2"],
    ["normalizeCronFileData", "0cdb1a33ff76b6a2e11ad7801a46acebe57e9a0381cafe2a6a7297cdcf0bfb26"],
    ["readCronFile", "e3cad11829ce96fb277d5ba371e1dfa598901b2dd34c7dae214e02adf986907f"],
    ["writeCronFile", "297578235f1c923ab996ad65ca9998fecdd396209f316e8e66b33410d5764418"], ],
  "src/cron/run-log.ts": [
    ["CronRunLogEntry", "d55ddc955cecf08fdb65932967106a5a7e6d7e26ffe221cd297ed0d8a9c1fd50"],
    ["resolveCronRunLogPath", "3ee21e34e80b1134e29a281558a61f84d06f43fbaca4ac072428bff5d5e8c9a3"],
    ["appendCronRunLog", "104fa52d96781b06459644ac1f66f11e84d11936578184070abae3861a785b45"],
    ["parseAllRunLogEntries", "aad0313278c39299cbf9c4102cc66694ddb54e4c1c92b352cbd3fa375392438c"],
  ], "src/channels/config.ts": [
    ["getChannelConfigPath", "6cb3e4c7cb52c574dfbabdd124ab4499bbb7decd9aa394106f5fd08ed588e0b3"],
    ["getChannelAccountsPath", "6eceb49c6e8bddf12205794d18635946752dd4ce81f74c712c03af31d8c6e24c"],
    ["getChannelRoutingPath", "4a0a5f999dd02d8317a123d8973bd29fca433a619bc672604842eb0d690b3230"],
    ["getChannelPairingPath", "4787563b228b2ea7033136b1bb52b0a40aeb56c2e5a4d098509948f10365b3a0"],
    ["getChannelTargetsPath", "83cf209733fdfd274d9973991f1bdeaa206f2716804c40fb7fe9cfdad9e779eb"], [
      "getPendingChannelControlRequestsPath",
      "f140356357a68ed3ad99a0e9bd554d9a21ebf40e50e9d9fae559a4c22d4a47a4", ],
    ["parseSimpleYaml", "a96768a6fa5a9e56ca956deccf4114bb58b76fd29a4f0321000684af087b7518"],
    ["telegramConfigCodec", "81db3a386572d2ed75c84880d13c42ae8d5046cff718d0e8d6709066080d9dc1"], ],
  "src/channels/accounts.ts": [
    ["ChannelAccountStore", "61025fd8fb3e05dcc46bf550073da457bf3c24089c70c3df3096d2977e1b3340"],
    ["getStore", "67d7082aee86c89998b658479629bcef73750799c79dfa200cea92b691f857b4"],
    ["loadChannelAccounts", "0153675d00759a7b26ab10b9e679524fdf44d5b2e0bc9b9d2caa040cb42f80c7"],
    ["saveChannelAccounts", "67b219224beb8e20580993f41b1e84048b9b606c33a31583fcc4db9f5836b033"], ],
  "src/channels/routing.ts": [
    ["loadRoutes", "5b36c4afcf00869abeb77bb940924a560347e744295e7d76940c45a3507000bb"],
    ["saveRoutes", "ffdd983c93f687f833306a50300cfa99f644d764cf3eb101cca282ddafbb2d02"], ],
  "src/channels/pairing.ts": [
    ["getStore", "2f594a440286aa3bd6dab9c066de1589d63349c18ba7403658db681f37f568b8"],
    ["loadPairingStore", "8b5e31b81ecd3fc5a999af2ba10de5e338a2c0a9c75842604708fdac54b0be82"],
    ["savePairingStore", "88e78eb823fec963a1b2decba554deb626a00d293df658470568f3b00d7d7670"], ],
  "src/channels/targets.ts": [
    ["ChannelTargetStore", "725d295faf3e73b003fee4042a23e9fccf4c3ae3b92a6be24b0643519ce2228d"],
    ["getStore", "086efbe458d3693010203de48c3c79c873ba3c6186987c347fb188c7e528ee5d"],
    ["loadTargetStore", "c7fa61d7aec8eede304fc7783bd98e4a0c60831d8b562ce3d5beafdceecea229"],
    ["saveTargetStore", "133d180625bdc01bdc63ab64f287582236a528be71b18debf2d2e59647c9cc84"], ],
  "src/channels/types.ts": [ [ "ChannelControlRequestEvent",
      "146565973c7380619e60d67e19472bcc52bd4252fa7880833fee374f7281b164", ],
    ["ChannelRoute", "43ae691cf9512923dacf2786059b7bc5f49bfc05cebf62f76d8527de58491036"],
    ["ChannelAccountBase", "b52ca6dc6be0e19c5705dff2b4ca6a00b6cd993c282c29428c6c3cdf2f318d78"],
    ["TelegramChannelAccount", "f38548b43c17c290efead604f32ffb52e967717cc5131cd9734a3993a67de93e"],
    ["PendingPairing", "455e2ec181a9710b13ad0cec58903fdb8d4ec1485b121aa5f0fc36572e2003d2"],
    ["ApprovedUser", "db392b1c19f37c48649ddefbb99a8995f5b68820846fd23ca0e6eb6aed8ccb03"],
    ["PairingStore", "81941ce446250f7d944232cd620580ecac10565ef123ac9a64d162f043839af1"],
    ["ChannelBindableTarget", "d330836371fa886151cfda3a3047898a49fa1b56ea5b1869744532a9355d574e"],
    ["TelegramChannelConfig", "8b824ab073e78e7fe2533a7beab275c4a6e4ba7d005b5ff434e1c28229995195"],
  ], "src/channels/pending-control-requests.ts": [ [ "PendingControlRequestStore",
      "a5069c0e949f995ece1ba49d8c676c9154f599d65a92c42693126d804fe1ef66", ],
    ["EMPTY_STORE", "1986035f8f859226d2c0ca64f71490d248d0360b46ea5822957c5ff6463de235"], [
      "isChannelControlRequestEvent",
      "2632ef4e0cf22464139815cced134f60c7dc5a80421b3d5439ceb8f3e500cf02", ],
    ["ensureStoreLoaded", "e48710b2145d5cdf6e23a2d9543609abd70a702c062afa6cbb041357df9809cd"],
    ["saveStore", "3cfc87e2c3db51475346d3608179fd45671df16adb002a9733172b5c8eef9f47"], ], });

function fail(message) { throw new Error(`persistence fixture extraction failed: ${message}`); }

function sha(bytes) { return createHash("sha256").update(bytes).digest("hex"); }

function json(value) { return `${JSON.stringify(value, null, 2)}\n`; }

function jsonl(values) { return `${values.map((value) => JSON.stringify(value)).join("\n")}\n`; }

function key(value) { return Buffer.from(value).toString("base64url"); }

function manifest(schema = 2, format = "pi-session-entry-jsonl", provider = "pi-ai") { return {
    schema_version: schema, message_format: format, provider_stack: provider, created_at: TIME, }; }

function message(id, role, content, extra = {}) { return {
    id: id, role: role, content: content, timestamp: 9466848e5, metadata: {
      created_at: TIME, }, ...extra, }; }

function entry(id, parentId, value) { return {
    type: "message", id: id, parentId: parentId, timestamp: TIME, message: value, }; }

function conversation(ids) { return {
    id: CONVERSATION, agent_id: AGENT, archived: false, archived_at: null, created_at: TIME,
    updated_at: TIME, last_message_at: TIME, summary: "SANITIZED_FIXTURE_SUMMARY",
    in_context_message_ids: ids, }; }

function agentRecord() { return {
    id: AGENT, name: "SANITIZED_FIXTURE_AGENT", description: "SANITIZED_FIXTURE_DESCRIPTION",
    system: "SANITIZED_FIXTURE_SYSTEM", tags: ["SANITIZED_FIXTURE_TAG"], model: "fixture/model",
    model_settings: {}, }; }

function prompt() { return {
    content: "SANITIZED_FIXTURE_SYSTEM_PROMPT", coreMemory: "SANITIZED_FIXTURE_MEMORY",
    compiledAt: TIME, rawSystemHash: "SANITIZED_FIXTURE_HASH", }; }

function session() { return { type: "session", version: 3, id: CONVERSATION, timestamp: TIME,
    cwd: "/SANITIZED_FIXTURE_WORKSPACE", }; }

function put(files, path, text) { if (files.has(path)) fail(`duplicate generated path ${path}`);
  if ( !path ||
    path.startsWith("/") ||
    path.split("/").some((part) => !part || part === "." || part === "..") ) {
    fail(`unsafe generated path ${path}`); }
  files.set(path, Buffer.from(text)); }

function baseline(files, root, rows, inputManifest = manifest(), ids = ["msg-user-fixture"]) {
  const defaultKey = key(`default:${AGENT}`);
  put(files, `${root}/agents/${key(AGENT)}.json`, json(agentRecord()));
  put(files, `${root}/conversations/${defaultKey}/conversation.json`, json(conversation(ids)));
  put(files, `${root}/conversations/${defaultKey}/manifest.json`, json(inputManifest));
  put(files, `${root}/conversations/${defaultKey}/messages.jsonl`, rows);
  put(files, `${root}/conversations/${defaultKey}/system-prompt.json`, json(prompt())); }

function expectation(files, root, behavior, mutation, indexedFiles) { put(
    files, `${root}/expectation.json`, json({
      behavior: behavior, mutation: mutation, indexed_files: indexedFiles, }), ); }

function addCurrent(files) { const root = CASE_NAMES[0];
  const user = message("msg-user-fixture", "user", [ {
      type: "text", text: "SANITIZED_FIXTURE_CURRENT", }, ]);
  baseline(files, root, jsonl([session(), entry("entry-user-fixture", null, user)]));
  const named = `${root}/conversations/${key(`conversation:${CONVERSATION}`)}`;
  put(files, `${named}/conversation.json`, json(conversation([user.id])));
  put(files, `${named}/manifest.json`, json(manifest()));
  put( files, `${named}/messages.jsonl`,
    jsonl([session(), entry("entry-named-fixture", null, user)]), );
  put(files, `${named}/system-prompt.json`, json(prompt()));
  expectation(files, root, "rust_reads", "none", ["agents", "conversations"]); }

function addRustTarget(files) { const root = CASE_NAMES[1];
  const user = message("msg-user-fixture", "user", [ {
      type: "text", text: "SANITIZED_FIXTURE_RUST_TARGET", }, ]);
  baseline(files, root, jsonl([session(), entry("entry-rust-fixture", null, user)]));
  expectation(files, root, "typescript_reads_exact_wire_keys", "none", ["agents", "conversations"]);
}

function addLegacy(files) { const root = CASE_NAMES[2];
  const legacy = { id: "ui-msg-fixture", role: "user", parts: [ {
        type: "text", text: "SANITIZED_FIXTURE_LEGACY_UI", }, ], metadata: { created_at: TIME, }, };
  const base = `${root}/conversations/${key(`default:${AGENT}`)}`;
  put(files, `${root}/agents/${key(AGENT)}.json`, json(agentRecord()));
  put(files, `${base}/conversation.json`, json(conversation([legacy.id])));
  put(files, `${base}/messages.jsonl`, jsonl([legacy]));
  put(files, `${base}/system-prompt.json`, json(prompt()));
  expectation(files, root, "transcript_migration_required", "backup_then_explicit_conversion", [
    "agents", "conversations", ]);
  const versioned = CASE_NAMES[3];
  const local = message("msg-legacy-fixture", "user", [ {
      type: "text", text: "SANITIZED_FIXTURE_VERSIONED_LEGACY", }, ]);
  baseline(files, versioned, jsonl([local]), manifest(1, "pi-ai-message-jsonl"), [local.id]);
  expectation(
    files, versioned, "load_then_upgrade_on_nonempty_persistence", "backup_and_schema2_rewrite",
    ["agents", "conversations"], ); }

function addTolerated(files) { const root = CASE_NAMES[4];
  const value = message("msg-tolerated-fixture", "user", [ {
      type: "text", text: "SANITIZED_FIXTURE_TOLERATED", }, ]);
  const rows = [session(), entry("entry-tolerated-fixture", null, value)];
  baseline(files, root, jsonl(rows), manifest(), [value.id]);
  expectation(files, root, "loader_ignores_session_header_and_loads_message_entry", "none", [
    "agents", "conversations", ]); }

function addOrphan(files) { const root = CASE_NAMES[5];
  const call = message( "msg-assistant-fixture", "assistant", [ {
        type: "toolCall", id: "call-valid-fixture", name: "fixture_tool", arguments: {
          marker: "SANITIZED_FIXTURE_CALL", }, }, ], {
      api: "fixture-api", provider: "fixture-provider", model: "fixture-model", usage: {
        input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: {
          input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0, }, },
      stopReason: "toolUse", }, );
  const valid = message( "msg-result-valid-fixture", "toolResult", [ {
        type: "text", text: "SANITIZED_FIXTURE_VALID_RESULT", }, ], {
      toolCallId: "call-valid-fixture", toolName: "fixture_tool", isError: false, }, );
  const orphan = message( "msg-result-orphan-fixture", "toolResult", [ {
        type: "text", text: "SANITIZED_FIXTURE_ORPHAN_RESULT", }, ], {
      toolCallId: "call-orphan-fixture", toolName: "fixture_tool", isError: false, }, );
  baseline( files, root, jsonl([ session(), entry("entry-call-fixture", null, call),
      entry("entry-valid-result-fixture", "entry-call-fixture", valid),
      entry("entry-orphan-result-fixture", "entry-valid-result-fixture", orphan), ]), manifest(),
    [call.id, valid.id, orphan.id], );
  put( files, `${root}/expected-active-projection.json`, json({
      message_ids: [call.id, valid.id], removed_message_ids: [orphan.id], transcript_mutated: false,
      conversation_mutated: true, }), );
  expectation( files, root, "orphan_removed_from_active_projection",
    "conversation_ids_repaired_source_unchanged",
    ["agents", "conversations", "expected-active-projection.json"], ); }

function addInterrupted(files) { const appendRoot = CASE_NAMES[6];
  const user = message("msg-user-fixture", "user", [ {
      type: "text", text: "SANITIZED_FIXTURE_APPEND_PREFIX", }, ]);
  const prefix = jsonl([session(), entry("entry-prefix-fixture", null, user)]);
  const truncated = `${prefix}{"type":"message","id":"entry-truncated-fixture",` +
    `"marker":"SANITIZED_FIXTURE_TRUNCATED"`;
  baseline(files, `${appendRoot}/input`, truncated, manifest(), [user.id]);
  put(files, `${appendRoot}/expected/complete-prefix.jsonl`, prefix);
  expectation(
    files, appendRoot, "lotta_hardening_recovers_complete_jsonl_prefix", "discard_truncated_tail",
    ["input", "expected"], );
  const root = CASE_NAMES[7];
  const active = message("msg-active-fixture", "user", [ {
      type: "text", text: "SANITIZED_FIXTURE_ACTIVE_ORIGINAL", }, ]);
  const candidate = message("msg-candidate-fixture", "user", [ {
      type: "text", text: "SANITIZED_FIXTURE_COMPLETE_CANDIDATE", }, ]);
  baseline( files, `${root}/input`, jsonl([session(), entry("entry-active-fixture", null, active)]),
    manifest(), [active.id], );
  baseline( files, `${root}/candidate`,
    jsonl([session(), entry("entry-candidate-fixture", null, candidate)]), manifest(),
    [candidate.id], );
  put( files, `${root}/expected/active-messages.jsonl`,
    jsonl([session(), entry("entry-active-fixture", null, active)]), );
  expectation(
    files, root, "lotta_hardening_preserves_canonical_original", "candidate_not_committed",
    ["input", "candidate", "expected"], ); }

function addCorrupt(files) { const root = CASE_NAMES[8];
  const rows = jsonl([ session(), entry( "entry-corrupt-fixture", null,
      message("msg-corrupt-fixture", "user", [ {
          type: "text", text: "SANITIZED_FIXTURE_CORRUPT_CASE", }, ]), ), ]);
  for (const [name, text, reason] of [ [
      "corrupt_json", '{"schema_version":2,"marker":"SANITIZED_FIXTURE_CORRUPT"',
      "malformed_manifest", ], ["unsupported_schema", json(manifest(99)), "unsupported_schema"], [
      "unsupported_provider",
      json(manifest(2, "pi-session-entry-jsonl", "fixture-unsupported-provider")),
      "unsupported_provider", ], ]) {
    const base = `${root}/${name}/input/conversations/${key(`default:${AGENT}`)}`;
    put(files, `${base}/manifest.json`, text);
    put(files, `${base}/messages.jsonl`, rows);
    put(files, `${root}/${name}/expected/active-bytes`, rows);
    put( files, `${root}/${name}/expected/expectation.json`, json({
        behavior: "reject", reason: reason, active_bytes_preserved: true, }), ); }
  put( files, `${root}/expectation.json`, json({ behavior: "reject_all_subcases", mutation: "none",
      indexed_files: ["corrupt_json", "unsupported_schema", "unsupported_provider"], }), ); }

function addSideStores(files) { put( files, SIDE_PATHS[0], json({
      lastAgent: null, tokenStreaming: false, reasoningTabCycleEnabled: false,
      showCompactions: false, conversationSwitchAlertEnabled: false, sessionContextEnabled: true,
      autoConversationTitles: false, autoConversationTitlesRollbackApplied: true,
      autoSwapOnQuotaLimit: true, includeWorktreeTool: true, recentModels: [],
      memoryReminderInterval: 25, reflectionTrigger: "step-count", reflectionStepCount: 25,
      reflectionMerge: "auto", reflectionMergeInstructions: "", globalSharedBlockIds: [], }), );
  put( files, SIDE_PATHS[1], json({ version: 1, scheduler_owner: null, tasks: [ {
          id: "schedule-fixture", agent_id: AGENT, conversation_id: CONVERSATION,
          name: "SANITIZED_FIXTURE_SCHEDULE", description: "SANITIZED_FIXTURE_DESCRIPTION",
          cron: "0 0 * * *", timezone: "UTC", recurring: true,
          prompt: "SANITIZED_FIXTURE_SCHEDULE_PROMPT", status: "active", created_at: TIME,
          expires_at: null, last_fired_at: null, fire_count: 0, cancel_reason: null,
          jitter_offset_ms: 0, last_run_at: null, last_run_outcome: null, last_run_reason: null,
          last_run_error: null, last_missed_at: null, missed_count: 0, failed_count: 0,
          scheduled_for: null, fired_at: null, missed_at: null, }, ], }), );
  put( files, SIDE_PATHS[2], jsonl([ {
        ts: 9466848e5, jobId: "schedule-fixture", action: "finished", status: "ok",
        outcome: "queued", reason: "scheduled_time_matched", summary: "SANITIZED_FIXTURE_RUN",
        agentId: AGENT, conversationId: CONVERSATION, }, ]), );
  put( files, SIDE_PATHS[3], json({ version: 1, providers: { "fixture-provider": {
          id: "local-provider-fixture-provider", name: "fixture-provider", provider_type: "openai",
          provider_category: "byok", auth: {
            type: "api", key: PLACEHOLDER, }, base_url: "https://fixture.invalid/v1",
          created_at: TIME, updated_at: TIME, }, }, }), );
  put( files, SIDE_PATHS[4], json({ requests: [], }), );
  put( files, SIDE_PATHS[5], [
      "enabled: false", "token: <redacted-fixture>", "dm_policy: pairing", "allowed_users: []",
      "group_mode: open", "transcribe_voice: false", "rich_private_chat_default: true",
      "rich_draft_streaming: false", "", ].join("\n"), );
  put( files, SIDE_PATHS[6], json({ accounts: [], }), );
  put( files, SIDE_PATHS[7], json({ routes: [], }), );
  put( files, SIDE_PATHS[8], json({ pending: [], approved: [], }), );
  put( files, SIDE_PATHS[9], json({ targets: [], }), );
  put(files, SIDE_PATHS[10], json({}));
  put( files, SIDE_PATHS[11], json({ lastAgent: AGENT, }), ); }

function index(files) { const obligations = [1, 2, 3, 3, 4, 5, 6, 6, 7];
  const formats = [ "schema2-pi-session-entry-jsonl", "schema2-pi-session-entry-jsonl",
    "unversioned-legacy-ui-jsonl", "schema1-pi-ai-message-jsonl", "schema2-tolerated-rows",
    "schema2-orphan-tool-result", "schema2-truncated-jsonl", "schema2-replacement-evidence",
    "corrupt-and-unsupported-manifests", ];
  const rust = [ "reads_current", "writes_exact_wire", "requires_explicit_migration",
    "upgrades_on_nonempty_persistence", "loads_message_entries", "repairs_active_projection",
    "recovers_complete_prefix", "preserves_original", "rejects_without_mutation", ];
  const typescript = [ "writes_current", "reads_exact_wire", "requires_explicit_migration",
    "upgrades_on_nonempty_persistence", "ignores_session_header", "repairs_active_projection",
    "not_applicable_rust_hardening", "not_applicable_rust_hardening", "rejects_without_mutation", ];
  const mutations = [
    "none", "none", "backup_then_conversion", "schema2_rewrite", "none", "conversation_only",
    "discard_truncated_tail", "candidate_not_committed", "none", ];
  return {
    schema_version: 1, source_commit: PIN, generator: "tools/extract-persistence-fixtures.mjs",
    derivation:
      "Source-derived deterministic Node harness reproducing pinned persisted " +
      "layouts and formats without importing runtime packages.", placeholder_policy:
      "All content is synthetic SANITIZED_FIXTURE_* data. Credential-like " +
      `fields may contain only ${PLACEHOLDER} or not-needed.`, ids: {
      agent: AGENT, conversation: CONVERSATION, }, keys: { agent: {
        source: AGENT, encoded: key(AGENT), }, default_conversation: {
        source: `default:${AGENT}`, encoded: key(`default:${AGENT}`), }, named_conversation: {
        source: `conversation:${CONVERSATION}`, encoded: key(`conversation:${CONVERSATION}`), }, },
    cases: CASE_NAMES.map((name, position) => ({
      case_id: position + 1, obligation: obligations[position], relative_root: name,
      source_format: formats[position], expected_rust_behavior: rust[position],
      expected_typescript_behavior: typescript[position],
      expected_recovery_or_mutation: mutations[position], })), intentional_invalid_files: [
      "interrupted_append/input/conversations/" +
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/messages.jsonl",
      "corrupt_unsupported_manifests/corrupt_json/input/conversations/" +
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json",
      "corrupt_unsupported_manifests/unsupported_schema/input/conversations/" +
        "ZGVmYXVsdDphZ2VudC1sb2NhbC1maXh0dXJl/manifest.json", ],
    side_stores: SIDE_PATHS.map((path) => ({ path: path,
      format: path.endsWith(".jsonl") ? "jsonl" : path.endsWith("config.yaml") ? "yaml" : "json",
    })), }; }

function corpus(sourceRegions) { const files = new Map();
  addCurrent(files);
  addRustTarget(files);
  addLegacy(files);
  addTolerated(files);
  addOrphan(files);
  addInterrupted(files);
  addCorrupt(files);
  addSideStores(files);
  if (sourceRegions) assertDerivedFields(sourceRegions, files);
  const metadata = index(files);
  metadata.inventory_schema =
    "Every corpus file except self-referential index.json is listed by sorted " +
    "path, semantic kind, byte count, and SHA-256. index.json is additionally " +
    "included in the complete loader and sanitization scan.";
  metadata.inventory = [...files.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([path, bytes]) => ({ path: path, semantic_kind: path.endsWith(".jsonl") ? "jsonl"
        : path.endsWith(".json") ? "json" : path.endsWith(".yaml") ? "yaml_or_json"
            : "bytes", byte_count: bytes.length, sha256: sha(bytes), }));
  put(files, "index.json", json(metadata));
  validateFiles(files);
  scanFiles(files);
  return files; }

function validateFiles(files) { let bytes = 0;
  if (files.size > LIMITS.files) fail(`file bound exceeded: ${files.size}/${LIMITS.files}`);
  for (const [path, value] of files) { bytes += value.length;
    if (path.split("/").length > LIMITS.depth) fail(`path depth bound exceeded: ${path}`); }
  if (bytes > LIMITS.bytes) fail(`byte bound exceeded: ${bytes}/${LIMITS.bytes}`); }

function secretViolation(text) { const lower = text.toLowerCase();
  const patterns = [
    ["secret prefix", ["s", "k", "-"].join("")], ["authorization", ["bear", "er "].join("")],
    ["private key", ["-----begin ", "private key-----"].join("")],
    ["common token", ["g", "hp_"].join("")], ["common token", ["x", "oxb-"].join("")],
    ["cloud credential", ["a", "kia"].join("")], ];
  for (const [label, value] of patterns) if (lower.includes(value)) return label;
  if (/[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/.test(text)) return "jwt shape";
  const fieldName = "api[_-]?key|secret|token|password|access|refresh";
  const field = new RegExp(`["']?(${fieldName})["']?\\s*[:=]\\s*["']?([^"'\\s,}\\n]+)["']?`, "gi");
  for (const match of text.matchAll(field)) {
    if (match[2] !== PLACEHOLDER && match[2] !== "not-needed") return "secret-like field"; }
  return undefined; }

function scanFiles(files) { for (const [path, bytes] of files) {
    const violation = secretViolation(bytes.toString("utf8"));
    if (violation) fail(`sanitization ${violation} in ${path}`); } }

function lexicalMask(text) { const mask = [...text];
  let state = "code";
  let quote = "";
  for (let index = 0; index < text.length; index++) { const value = text[index];
    const next = text[index + 1];
    if (state === "code" && value === "/" && next === "/") { mask[index] = mask[index + 1] = " ";
      index++;
      state = "line"; } else if (state === "code" && value === "/" && next === "*") {
      mask[index] = mask[index + 1] = " ";
      index++;
      state = "block"; } else if (state === "code" && ["'", '"', "`"].includes(value)) {
      quote = value;
      mask[index] = " ";
      state = "string"; } else if (state === "line") {
      if (value.charCodeAt(0) === 10) state = "code";
      else mask[index] = " "; } else if (state === "block") { mask[index] = " ";
      if (value === "*" && next === "/") { mask[index + 1] = " ";
        index++;
        state = "code"; } } else if (state === "string") { mask[index] = " ";
      if (value === "\\") { if (index + 1 < text.length) mask[++index] = " ";
      } else if (value === quote) state = "code"; } }
  if (state === "block" || state === "string") fail("unterminated lexical region");
  return mask.join(""); }

function declarationStarts(mask, name) { const matches = [];
  for (const kind of ["const", "interface", "type", "function", "class"]) {
    for (const exported of ["", "export "]) { const marker = `${exported}${kind} ${name}`;
      let at = mask.indexOf(marker);
      while (at >= 0) { const line = mask.lastIndexOf(String.fromCharCode(10), at - 1) + 1;
        const before = at === line ? "" : mask[at - 1];
        const after = mask[at + marker.length];
        const boundary = !before || !/[A-Za-z0-9_$]/.test(before);
        const exactEnd = !after || !/[A-Za-z0-9_$]/.test(after);
        const prefix = mask.slice(line, at).trim();
        const cleanedPrefix = prefix.replace(/^\/\/\s*/, "");
        const validPrefix = cleanedPrefix === "" || (exported === "" && cleanedPrefix === "export");
        if (boundary && exactEnd && validPrefix)
          matches.push({ index: at, 0: mask.slice(at, at + marker.length), });
        at = mask.indexOf(marker, at + marker.length); } } }
  for (const visibility of ["private", "public", "protected"]) {
    const marker = `${visibility} ${name}`;
    let at = mask.indexOf(marker);
    while (at >= 0) { const line = mask.lastIndexOf(String.fromCharCode(10), at - 1) + 1;
      if (mask.slice(line, at).trim() === "")
        matches.push({ index: at, 0: mask.slice(at, at + marker.length), });
      at = mask.indexOf(marker, at + marker.length); } }
  matches.sort((left, right) => left.index - right.index);
  return matches.filter( (match, position) =>
      position === 0 || !mask.slice(matches[position - 1].index, match.index).includes("export "),
  ); }

function operativeRegion(text, name) { const mask = lexicalMask(text);
  const matches = declarationStarts(mask, name);
  if (matches.length !== 1) fail(`operative marker ${name} count ${matches.length}`);
  const start = matches[0].index;
  const afterName = start + matches[0][0].length;
  const brace = mask.indexOf("{", afterName);
  const semicolon = mask.indexOf(";", afterName);
  if (semicolon >= 0 && (brace < 0 || semicolon < brace)) return text.slice(start, semicolon + 1);
  if (brace < 0) fail(`unterminated operative region ${name}`);
  let depth = 0;
  for (let index = brace; index < mask.length; index++) { if (mask[index] === "{") depth++;
    if (mask[index] === "}" && --depth === 0) { let end = index + 1;
      while (mask[end] === " " || mask[end] === "\t") end++;
      if (mask[end] === ";") end++;
      return text.slice(start, end); } }
  fail(`unterminated operative region ${name}`); }

function interfaceFields(region) { const body = lexicalMask(region);
  const open = body.indexOf("{");
  const close = body.lastIndexOf("}");
  if (open < 0 || close <= open) fail("interface field region invalid");
  const required = [];
  const optional = [];
  let depth = 0;
  let statement = "";
  const consume = () => { const match = statement.match(/(?:^|\n)\s*([A-Za-z_$][\w$]*)(\?)?\s*:/);
    if (match) (match[2] ? optional : required).push(match[1]);
    statement = ""; };
  for (const value of body.slice(open + 1, close)) {
    if (value === "{" || value === "(" || value === "[") depth++;
    if (value === "}" || value === ")" || value === "]") depth--;
    if (value === ";" && depth === 0) consume();
    else statement += value; }
  consume();
  return { required: required, optional: optional, }; }

function assertObjectFields(value, fields, label) { const actual = Object.keys(value).sort();
  for (const field of fields.required)
    if (!(field in value)) fail(`${label} missing source field ${field}`);
  const supported = [...fields.required, ...fields.optional].sort();
  if (actual.some((field) => !supported.includes(field))) fail(`${label} unsupported field`); }

function assertDerivedFields(regions, files) { const manager = regions["src/settings-manager.ts"];
  const globalFields = interfaceFields(manager.Settings);
  const legacySettings = interfaceFields(regions["src/settings.ts"].Settings);
  globalFields.optional.push(...legacySettings.required, ...legacySettings.optional);
  assertObjectFields(JSON.parse(files.get(SIDE_PATHS[0])), globalFields, "global settings");
  assertObjectFields(
    JSON.parse(files.get(SIDE_PATHS[10])), interfaceFields(manager.ProjectSettings),
    "project settings", );
  assertObjectFields(
    JSON.parse(files.get(SIDE_PATHS[11])), interfaceFields(manager.LocalProjectSettings),
    "local settings", );
  const cron = JSON.parse(files.get(SIDE_PATHS[1]));
  assertObjectFields( cron.tasks[0], interfaceFields(regions["src/cron/cron-file.ts"].CronTask),
    "cron task", );
  const run = JSON.parse(files.get(SIDE_PATHS[2]).toString().trim());
  assertObjectFields( run, interfaceFields(regions["src/cron/run-log.ts"].CronRunLogEntry),
    "cron run", );
  const auth = JSON.parse(files.get(SIDE_PATHS[3]));
  assertObjectFields( auth, interfaceFields(
      regions["src/backend/local/local-provider-auth-store.ts"].LocalProviderAuthFile, ),
    "auth file", );
  assertObjectFields( auth.providers["fixture-provider"],
    interfaceFields(regions["src/backend/local/local-provider-auth-store.ts"].LocalProviderRecord),
    "provider record", ); }

function assertOperativeEvidence(source, checkWholeHashes = true, generatedFiles) {
  const regions = {};
  for (const [path, declarations] of Object.entries(OPERATIVE)) { let bytes;
    try { bytes = readFileSync(resolve(source, path)); } catch {
      fail(`source evidence missing: ${path}`); }
    if (checkWholeHashes && sha(bytes) !== SOURCES[path]) fail(`source hash drift: ${path}`);
    const text = bytes.toString("utf8");
    regions[path] = {};
    for (const [name, expectedHash] of declarations) { const region = operativeRegion(text, name);
      if (sha(Buffer.from(region)) !== expectedHash)
        fail(`operative region drift: ${path}#${name}`);
      regions[path][name] = region; } }
  if (generatedFiles) assertDerivedFields(regions, generatedFiles);
  return regions; }

function assertEvidence(source, generatedFiles) {
  return assertOperativeEvidence(source, true, generatedFiles); }

function isCheckout(path) { try {
    execFileSync("git", ["-C", path, "rev-parse", "--is-inside-work-tree"], { stdio: "ignore", });
    return true; } catch { return false; } }

function discoverSource(repoRoot, explicit, available = isCheckout) {
  if (explicit !== undefined) return resolve(explicit);
  const sibling = resolve(repoRoot, "..", "letta-code");
  const workspaceSibling = resolve(repoRoot, "..", "..", "letta-code");
  const child = resolve(repoRoot, "letta-code");
  if (available(sibling)) return sibling;
  if (available(workspaceSibling)) return workspaceSibling;
  if (available(child)) return child;
  fail( "source checkout not found; checked ../letta-code, ../../letta-code, " +
      "and ./letta-code; pass --source <checkout>", ); }

function verifyPin( source, head = () => execFileSync("git", ["-C", source, "rev-parse", "HEAD"], {
      encoding: "utf8", }).trim(), ) { const actual = head();
  if (actual !== PIN) fail(`source HEAD ${actual} does not match pinned ${PIN}`); }

function listTree(root) { const files = new Map();
  const stack = [ { path: root, depth: 0, }, ];
  while (stack.length > 0) { const item = stack.pop();
    if (item.depth > LIMITS.depth) fail("existing tree depth bound exceeded");
    const entries = readdirSync(item.path, { withFileTypes: true,
    }).sort((a, b) => b.name.localeCompare(a.name));
    for (const entry of entries) { const path = resolve(item.path, entry.name);
      const rel = relative(root, path).split(sep).join("/");
      if (entry.isSymbolicLink()) fail(`existing tree symlink rejected: ${rel}`);
      if (entry.isDirectory())
        stack.push({ path: path, depth: item.depth + 1, });
      else if (entry.isFile()) files.set(rel, readFileSync(path));
      else fail(`existing non-file rejected: ${rel}`);
      if (files.size + stack.length > LIMITS.files) fail("existing tree file bound exceeded"); } }
  validateFiles(files);
  return files; }

function compareTrees(expected, actual) { const diagnostics = [];
  for (const path of expected.keys()) { if (!actual.has(path)) diagnostics.push(`missing ${path}`);
    else if (!expected.get(path).equals(actual.get(path))) diagnostics.push(`different ${path}`); }
  for (const path of actual.keys()) if (!expected.has(path)) diagnostics.push(`extra ${path}`);
  diagnostics.sort();
  if (diagnostics.length > 0) { const listed = diagnostics.slice(0, LIMITS.diagnostics).join("; ");
    const suffix = diagnostics.length > LIMITS.diagnostics ? "; diagnostics truncated" : "";
    fail(`tree differs: ${listed}${suffix}`); } }

function writeTree(root, files) { for (const [path, bytes] of files) {
    const target = resolve(root, path);
    mkdirSync(dirname(target), { recursive: true, });
    writeFileSync(target, bytes, { mode: 420, }); } }

function replaceTree(files) {
  const tempParent = mkdtempSync(resolve(dirname(OUT), ".persistence-fixtures-"));
  const candidate = resolve(tempParent, "persistence");
  const backup = resolve(tempParent, "previous");
  mkdirSync(candidate);
  try { writeTree(candidate, files);
    try { renameSync(OUT, backup); } catch (error) { if (error.code !== "ENOENT") throw error; }
    renameSync(candidate, OUT);
    rmSync(backup, { recursive: true, force: true, }); } finally { rmSync(tempParent, {
      recursive: true, force: true, }); } }

function expectFailure(label, action, fragment) { try { action();
    fail(`self-test ${label} unexpectedly succeeded`); } catch (error) {
    if (!String(error.message).includes(fragment)) throw error; } }

function selfTest() {
  testBase64url();
  testDiscovery();
  testSanitization();
  testTreeComparison();
  testSourceEvidence();
  console.log("persistence extractor self-test passed");
}

function testBase64url() {
  for (const value of [AGENT, `default:${AGENT}`, `conversation:${CONVERSATION}`]) {
    const encoded = key(value);
    if (!/^[A-Za-z0-9_-]+$/.test(encoded) || /[=+/]/.test(encoded))
      fail("self-test base64url alphabet");
    if (Buffer.from(encoded, "base64url").toString() !== value)
      fail("self-test base64url roundtrip");
  }
}

function testDiscovery() {
  const fakeRoot = resolve(tmpdir(), "lotta-discovery-fixture");
  const sibling = resolve(fakeRoot, "..", "letta-code");
  const workspaceSibling = resolve(fakeRoot, "..", "..", "letta-code");
  const child = resolve(fakeRoot, "letta-code");
  const available = (set) => (path) => set.has(path);
  const allLayouts = available(new Set([sibling, workspaceSibling, child]));
  if (discoverSource(fakeRoot, undefined, allLayouts) !== sibling)
    fail("self-test sibling discovery");
  const workspaceLayouts = available(new Set([workspaceSibling, child]));
  if (discoverSource(fakeRoot, undefined, workspaceLayouts) !== workspaceSibling)
    fail("self-test workspace sibling discovery");
  if (discoverSource(fakeRoot, undefined, available(new Set([child]))) !== child)
    fail("self-test child discovery");
  expectFailure(
    "bounded discovery",
    () => discoverSource(fakeRoot, undefined, available(new Set())),
    "checked ../letta-code, ../../letta-code, and ./letta-code",
  );
  const explicit = resolve(fakeRoot, "explicit");
  let explicitAvailabilityChecked = false;
  const explicitResult = discoverSource(fakeRoot, explicit, () => {
    explicitAvailabilityChecked = true;
    return true;
  });
  if (explicitResult !== explicit || explicitAvailabilityChecked)
    fail("self-test explicit source must win without fallback");
}

function testSanitization() {
  const risky = [
    ["prefix", ["s", "k", "-", "fixture"].join("")],
    ["auth", ["Bear", "er fixture"].join("")],
    ["key", ["-----BEGIN ", "PRIVATE KEY-----"].join("")],
    ["token", ["g", "hp_fixture"].join("")],
    ["jwt", ["abcdefgh", "ijklmnop", "qrstuvwx"].join(".")],
    ["cloud", ["A", "KIAFIXTURE"].join("")],
    ["oauth access", JSON.stringify({ access: "fixture-value", })],
    ["oauth refresh", JSON.stringify({ refresh: "fixture-value", })],
    ["json field", JSON.stringify({ api_key: "fixture-value", })],
    ["yaml field", "password: fixture-value"],
  ];
  for (const [label, text] of risky)
    if (!secretViolation(text)) fail(`self-test sanitization ${label}`);
}

function testTreeComparison() {
  const expected = new Map([["a", Buffer.from("one")]]);
  expectFailure("missing", () => compareTrees(expected, new Map()), "missing a");
  expectFailure(
    "extra",
    () => compareTrees(expected, new Map([...expected, ["b", Buffer.from("two")]])),
    "extra b",
  );
  expectFailure(
    "different",
    () => compareTrees(expected, new Map([["a", Buffer.from("two")]])),
    "different a",
  );
  expectFailure("pin", () => verifyPin("fixture", () => "wrong"), "does not match pinned");
}

function testSourceEvidence() {
  const source = mkdtempSync(resolve(tmpdir(), "lotta-persistence-source-"));
  try {
    const baseline = discoverSource(ROOT);
    for (const path of Object.keys(SOURCES)) {
      const target = resolve(source, path);
      mkdirSync(dirname(target), { recursive: true, });
      writeFileSync(target, readFileSync(resolve(baseline, path)));
    }
    assertOperativeEvidence(source, false);
    const target = resolve(source, "src/backend/local/local-store.ts");
    const original = readFileSync(target, "utf8");
    const changed = original.replace('toString("base64url")', 'toString("base64")');
    const decoy = `${changed}\n// decoy function encodePathSegment(value: string) { `
      + `return Buffer.from(value).toString("base64url"); }\n`;
    writeFileSync(target, decoy);
    expectFailure(
      "operative drift with decoy",
      () => assertOperativeEvidence(source, false),
      "operative region drift",
    );
    writeFileSync(target, original.replace(
        "function encodePathSegment", "function encodePathSegment\nfunction encodePathSegment",
    ));
    expectFailure(
      "duplicate marker",
      () => assertOperativeEvidence(source, false),
      "operative marker encodePathSegment count 2",
    );
    const unterminated = original.replace(
      'return Buffer.from(value).toString("base64url");\n}',
      'return Buffer.from(value).toString("base64url");',
    );
    writeFileSync(target, unterminated);
    expectFailure(
      "unterminated region",
      () => assertOperativeEvidence(source, false),
      "unterminated operative region encodePathSegment",
    );
    writeFileSync(target, changed);
    expectFailure("whole file drift", () => assertEvidence(source), "source hash drift");
  } finally {
    rmSync(source, { recursive: true, force: true, });
  }
}

function parseArgs(args) { const allowed = new Set(["--source", "--check", "--self-test"]);
  for (const arg of args)
    if (arg.startsWith("--") && !allowed.has(arg)) fail(`unknown option ${arg}`);
  const at = args.indexOf("--source");
  if (at >= 0 && (!args[at + 1] || args[at + 1].startsWith("--")))
    fail("--source requires a checkout path");
  return { source: at >= 0 ? args[at + 1] : undefined, check: args.includes("--check"),
    selfTest: args.includes("--self-test"), }; }

function main() { const args = parseArgs(process.argv.slice(2));
  if (args.selfTest) return selfTest();
  const source = discoverSource(ROOT, args.source);
  verifyPin(source);
  const sourceRegions = assertEvidence(source);
  const files = corpus(sourceRegions);
  if (args.check) compareTrees(files, listTree(OUT));
  else replaceTree(files); }

try { main(); } catch (error) { console.error(error.message);
  process.exitCode = 1; }
