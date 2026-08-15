export const PIN = "300f923f16cc8eee50656d7da732902c1dea2b65";
export const BOUNDARY =
  "pinned AppServerClient + pinned source-test scenario boundary";
export const PLACEHOLDER = "<sanitized-trace>";
export const LIMITS = Object.freeze({
  files: 9,
  fileBytes: 131072,
  totalBytes: 524288,
  frames: 64,
  depth: 2,
  nameBytes: 96,
  supporting: 12,
});
export const SURFACES = Object.freeze([
  "queue",
  "abort",
  "disconnect",
  "stale-lease",
  "retry",
  "idempotency",
  "crash-recovery",
]);
export const INVARIANTS = Object.freeze([
  "increasing_event_seq_per_connection",
  "input_accepted_before_caused_events",
  "tool_start_before_matching_tool_end",
  "turn_finished_exactly_once_after_final_stream_delta",
  "no_server_event_from_stale_lease_after_replacement",
  "broadcast_delivery_stable_ascending_connection_ordinal",
]);
export const RULES = Object.freeze([
  {
    rule: "generated_uuid_alias",
    paths: [
      "/wire/agent_id",
      "/wire/conversation_id",
      "/wire/runtime/agent_id",
      "/wire/runtime/conversation_id",
      "/wire/turn_id",
      "/wire/run_id",
      "/wire/delta/id",
      "/wire/delta/tool_call_id",
      "/wire/queue/*/id",
      "/wire/loop_status/active_run_ids/*",
      "/wire/loop_status/executing_tool_call_ids/*",
    ],
  },
  {
    rule: "rfc3339_timestamp",
    paths: [
      "/wire/emitted_at",
      "/wire/delta/date",
      "/wire/queue/*/enqueued_at",
    ],
  },
  { rule: "event_seq_rank", paths: ["/wire/event_seq"] },
  {
    rule: "idempotency_key_unique_emission",
    paths: ["/wire/idempotency_key"],
  },
  { rule: "exact", paths: ["*"] },
]);
export const WHOLE = Object.freeze({
  "src/app-server-client.ts":
    "b9ca75d1f8a956b64fa5e3732a57683eccee1dda78b531648b2e963327de568a",
  "src/types/app-server-info.ts":
    "b75a09cee5b6aa2f7d19e8836e4ee36af289bc6f79d6de0594492b1c3d4a2433",
  "src/types/protocol_v2.ts":
    "bdcb0f02ba3c2e4788b4ac33e69747fb6c07c38c7f4587accb745670555bb52d",
  "src/types/runtime-scope.ts":
    "68c7f91118ca48592c166019a308a73e6ffeac0a24eec4273bee392a45fe19af",
  "src/types/queue-update-protocol.ts":
    "9ec2a11c02a5c6f5db5cf5fef8c738f318276baf5770f4bf6292c1b6133e10a2",
  "src/app-server-client.test.ts":
    "30e13e45728741ee85788fad91116166ee21535edba6956e5b2dc6ed73bcf4d4",
  "src/websocket/listener/queue-update-transitions.test.ts":
    "226413d0604696867628463617e71350a378abc385554d014138f80142a9294d",
  "src/websocket/listener/send-lease.test.ts":
    "0ef519f73a1cf27a6a913eac273d9274b7a4400e0297f2abfb989f340ba53df9",
  "src/websocket/listener/recovery-lease.test.ts":
    "52e440bacba4e2fbd3548c04c8555c63e141ba4bb6308455fa43a7c48c8b2063",
  "src/websocket/listener/recovery-sync.test.ts":
    "c27b26649defab74180b8a741258b6d5977c1a15b4b86dfea3d61f05f0f58547",
  "src/websocket/listener/turn-terminal-protocol.test.ts":
    "8343954ab94d1137e0c4b93066a5a4d3aab659a268a9f3fabe9c337678746019",
  "src/websocket/listener/connection-lifecycle.test.ts":
    "87d7b1d5b5fc7c1cc30a0dd77e44d94324d66361f07a5e44283f9a16a7e751d7",
  "src/websocket/listener/connection-state-sync.test.ts":
    "cce80147fb17dd8d9ba49e7d0b170ea3a463ff469dc0fb22ea1b7180b3298297",
  "src/websocket/listener/cloud-retry-message.test.ts":
    "b90ecd07db549f8a2d89f8e33d223fa0ceda48384159eeed26b4858f3443724c",
  "src/websocket/listener/turn-input-state.test.ts":
    "296e46f8564679fcea0a9f849e289d27e6021b948eddb88d21a9fab4df3e6979",
  "src/websocket/listener/protocol-ergonomics.test.ts":
    "a906931efddaeeb6dc457be282dce9218907b8cfc574135fd63ce043e00f6da9",
  "src/websocket/listener/protocol-outbound.test.ts":
    "1258fd5d21cbf66757dc4adf61c1a060fb7a6cf55b753ad6d8175225db1fb6f2",
  "src/websocket/listener/loop-state-executing-tools.test.ts":
    "006a9d3ca70105c07aed6f38353eb95e4f7674d93743c0859e054c053969b353",
  "src/websocket/listener/message-router.test.ts":
    "e47690da2c595de6c58365a4d5bf85611b5db2615af467d6ba2fecb85f17ec34",
  "src/websocket/listener/protocol-outbound.ts":
    "7f6ef43ec1e19c8d7be72c0e59b288af66d24f3b0da9dd3c910a3a9f9184b591",
});
export const REGION_SPECS = Object.freeze([
  ["src/app-server-client.ts", "AppServerClient"],
  ["src/app-server-client.ts", "attachSocketListener"],
  ["src/app-server-client.ts", "waitForSocketOpen"],
  ["src/app-server-client.ts", "method:send"],
  ["src/app-server-client.ts", "method:request"],
  ["src/app-server-client.ts", "method:runtimeStart"],
  ["src/app-server-client.ts", "method:submitInput"],
  ["src/app-server-client.ts", "method:abort"],
  ["src/app-server-client.ts", "method:sync"],
  ["src/app-server-client.ts", "method:handleMessage"],
  ["src/app-server-client.ts", "method:handleDisconnect"],
  ["src/types/app-server-info.ts", "isAppServerInfoResponseMessage"],
  ["src/types/protocol_v2.ts", "WsProtocolMessage"],
  ["src/types/runtime-scope.ts", "RuntimeScope"],
  ["src/types/queue-update-protocol.ts", "QueueRemovalTransition"],
  [
    "src/app-server-client.test.ts",
    "test:connects one socket and resolves request_id responses",
  ],
  [
    "src/app-server-client.test.ts",
    "test:notifies once when the websocket disconnects unexpectedly",
  ],
  [
    "src/app-server-client.test.ts",
    "test:wraps sync, abort, and input commands",
  ],
  [
    "src/websocket/listener/queue-update-transitions.test.ts",
    "test:active continuation dequeue emits exact message identities",
  ],
  [
    "src/websocket/listener/queue-update-transitions.test.ts",
    "test:explicit queue removal emits cancellation rather than dequeue",
  ],
  [
    "src/websocket/listener/send-lease.test.ts",
    "test:a reset during tool preparation cannot consume replacement input",
  ],
  [
    "src/websocket/listener/recovery-lease.test.ts",
    "test:stale recovered tool execution emits nothing into a replacement run",
  ],
  [
    "src/websocket/listener/recovery-sync.test.ts",
    "describe:recoverApprovalStateForSync restart recovery",
  ],
  [
    "src/websocket/listener/turn-terminal-protocol.test.ts",
    "test:finishListenerTurn emits exactly one correlated terminal event",
  ],
  [
    "src/websocket/listener/connection-lifecycle.test.ts",
    "test:connection cleanup preserves other subscribers",
  ],
  [
    "src/websocket/listener/connection-state-sync.test.ts",
    "test:keeps attached App Server connections from bypassing the startup barrier",
  ],
  [
    "src/websocket/listener/cloud-retry-message.test.ts",
    "test:normalizes Cloud retry metadata into the listener protocol",
  ],
  [
    "src/websocket/listener/turn-input-state.test.ts",
    "describe:listener turn input state",
  ],
  [
    "src/websocket/listener/protocol-ergonomics.test.ts",
    "test:abort_message sends an ack response when request_id is provided",
  ],
  [
    "src/websocket/listener/protocol-outbound.test.ts",
    "test:fans notifications out to subscribers and honors an explicit target",
  ],
  [
    "src/websocket/listener/loop-state-executing-tools.test.ts",
    "test:reuses the approval request message id for tool lifecycle rows",
  ],
  [
    "src/websocket/listener/loop-state-executing-tools.test.ts",
    "test:emits error-status client_tool_end events for every tool call id",
  ],
  [
    "src/websocket/listener/message-router.test.ts",
    "test:preserves the acting user on a directly-owned input and deduplicates retries",
  ],
  [
    "src/websocket/listener/message-router.test.ts",
    "test:a direct message that loses the idle race is queued and later drained",
  ],
  [
    "src/websocket/listener/protocol-outbound.ts",
    "emitProtocolV2Message",
  ],
]);
export const REGIONS = Object.freeze([
  [
    "src/app-server-client.ts",
    "AppServerClient",
    "f85533dc3ced2db0589b96c3ad5eef4af7aa26f6836cd90b912246119fc8975e",
  ],
  [
    "src/app-server-client.ts",
    "attachSocketListener",
    "7c2ca4db25b5a7bb7ea811ef50160a03dde64a1418ef551ddcdc1b03c48e416f",
  ],
  [
    "src/app-server-client.ts",
    "waitForSocketOpen",
    "ff9fc638a6429bdc50f760568d260c73cad99b1c7e21f158830091b680415037",
  ],
  [
    "src/app-server-client.ts",
    "method:send",
    "8ad03734459278f8669d628b3d71ff0fac2652cdc3a1face6166ced80a57ccf1",
  ],
  [
    "src/app-server-client.ts",
    "method:request",
    "35928632f4dc669975620f70f75ba19e11f0000b8c3953a9833dbe0097105cb4",
  ],
  [
    "src/app-server-client.ts",
    "method:runtimeStart",
    "700f61e123f3a3b76bde3ff2ec264e22154aeb734a99f38d745b5d97ae9894ff",
  ],
  [
    "src/app-server-client.ts",
    "method:submitInput",
    "03da10065b4b1bd7024b675b7f7d82b2235540d30fe8442ff0d37d4843cbec99",
  ],
  [
    "src/app-server-client.ts",
    "method:abort",
    "1acb52620b2096bc74cfdd22fec45efe42ccc22512cc0d03d0a4b09d676ed2a5",
  ],
  [
    "src/app-server-client.ts",
    "method:sync",
    "19360dd114c8a436fd16a0125840d4c34e71f1451611fa7aa55bc8898fa36d78",
  ],
  [
    "src/app-server-client.ts",
    "method:handleMessage",
    "499c9a5fd813d612a23f86952c07efea7b09d7cb3430434df2d6680c2e449e4f",
  ],
  [
    "src/app-server-client.ts",
    "method:handleDisconnect",
    "7478fbef5bf8c276718cdf3913b826bf91f71324f4cfd801e41d94b7977e2182",
  ],
  [
    "src/types/app-server-info.ts",
    "isAppServerInfoResponseMessage",
    "615c50645a4ff83270af1e73f5787b5b93d38b666b4ec6f14825b990184a9b9f",
  ],
  [
    "src/types/protocol_v2.ts",
    "WsProtocolMessage",
    "cc832fb7da90013feef225c5a7f95352fb479b4d2f8e86d09f0052723ca3cf34",
  ],
  [
    "src/types/runtime-scope.ts",
    "RuntimeScope",
    "1da2c1847a223e17783f85e89fdcfb887551bcc11c54c3c4bfe1c88a5e622b0a",
  ],
  [
    "src/types/queue-update-protocol.ts",
    "QueueRemovalTransition",
    "cba7452dd25261fd938bb4a62877b3131ba4d6d833e3b92c229224d1522681e9",
  ],
  [
    "src/app-server-client.test.ts",
    "test:connects one socket and resolves request_id responses",
    "025617c572432fe4b323cab4b3d2c9322ae0c335d089207411fbeec82aca1522",
  ],
  [
    "src/app-server-client.test.ts",
    "test:notifies once when the websocket disconnects unexpectedly",
    "4632e1b8d129f2c6ec17aef4444972ab72a96e852596cb072dcf67857fd65495",
  ],
  [
    "src/app-server-client.test.ts",
    "test:wraps sync, abort, and input commands",
    "9a5f43105ae102b13805b525308ca85f20dc333c826f1ccab68ac95a8103b537",
  ],
  [
    "src/websocket/listener/queue-update-transitions.test.ts",
    "test:active continuation dequeue emits exact message identities",
    "2607fb2c4010fd2ce51ea684d9f531a43b2c0d22de4e1d76fbe98f20ab53142d",
  ],
  [
    "src/websocket/listener/queue-update-transitions.test.ts",
    "test:explicit queue removal emits cancellation rather than dequeue",
    "20dce27e0d5c037c493b61bca8766594bdc89d51d2cb6e1bd2745e65d7c522e0",
  ],
  [
    "src/websocket/listener/send-lease.test.ts",
    "test:a reset during tool preparation cannot consume replacement input",
    "e7802c4c5f0ebc0e5d12eef664472e25da5e582e458e02861d9bec95c0272589",
  ],
  [
    "src/websocket/listener/recovery-lease.test.ts",
    "test:stale recovered tool execution emits nothing into a replacement run",
    "4df7964021fcc29723001d11bbc757c70e939633b434321d0eb0e71a4357e12a",
  ],
  [
    "src/websocket/listener/recovery-sync.test.ts",
    "describe:recoverApprovalStateForSync restart recovery",
    "18ae24c7b203e20e793b46138f90affaea4a171892f0ad7d2502d1aba97e432e",
  ],
  [
    "src/websocket/listener/turn-terminal-protocol.test.ts",
    "test:finishListenerTurn emits exactly one correlated terminal event",
    "e687804bfacf5256a6e4304c4b919b05f9d96bd49feb8819f1e780c910e87e5a",
  ],
  [
    "src/websocket/listener/connection-lifecycle.test.ts",
    "test:connection cleanup preserves other subscribers",
    "e9caad61334ec0c229b9e886a7be70fedca48b11e2e7294c4640d3c42a6adeb8",
  ],
  [
    "src/websocket/listener/connection-state-sync.test.ts",
    "test:keeps attached App Server connections from bypassing the startup barrier",
    "9428a335aeeb947cc49f7345d7f7b5530c0a81050f1d3ccf2c19fcb824a4044b",
  ],
  [
    "src/websocket/listener/cloud-retry-message.test.ts",
    "test:normalizes Cloud retry metadata into the listener protocol",
    "0e54fab5cf1219bf7e6293f843ebcffdd20888c284b36f9c417fe4f16cce3ea8",
  ],
  [
    "src/websocket/listener/turn-input-state.test.ts",
    "describe:listener turn input state",
    "36b4fe2f3f0e638cdcd2b171fb3b30c3b067478bb60ae6bb4026958a0629850c",
  ],
  [
    "src/websocket/listener/protocol-ergonomics.test.ts",
    "test:abort_message sends an ack response when request_id is provided",
    "91817dabeb466ec20595666b06b0353573cd331348c107c14ff5cb3c175a67c7",
  ],
  [
    "src/websocket/listener/protocol-outbound.test.ts",
    "test:fans notifications out to subscribers and honors an explicit target",
    "710ea6d9a81e93ef85dae496fe353e2b8b4a2c401d0e5b1d116ab8ab7bbde67d",
  ],
  [
    "src/websocket/listener/loop-state-executing-tools.test.ts",
    "test:reuses the approval request message id for tool lifecycle rows",
    "ce01e64a280d1e8597d9f7d8baed0ded1b2b01670c2ac6c706870fdea7c757e2",
  ],
  [
    "src/websocket/listener/loop-state-executing-tools.test.ts",
    "test:emits error-status client_tool_end events for every tool call id",
    "1032d500d8e24659b818ec1ed85c9c4446cc21db6252825221b0e406f4cf1edf",
  ],
  [
    "src/websocket/listener/message-router.test.ts",
    "test:preserves the acting user on a directly-owned input and deduplicates retries",
    "1d4d6e9772bd614bb1a3526f7c6a27a10efd6d8f71a1e294126c2ce7179d36e6",
  ],
  [
    "src/websocket/listener/message-router.test.ts",
    "test:a direct message that loses the idle race is queued and later drained",
    "f2994a4e73c7f7d14744f905375a2bd2044081e458a58b15ef79eccceeccd880",
  ],
  [
    "src/websocket/listener/protocol-outbound.ts",
    "emitProtocolV2Message",
    "56242ed103e3105b7ca7d0061a3388e405c962e9597cef874ddd9e54db401187",
  ],
]);

export const SUPPORT = Object.freeze({
  queue: [
    [
      "src/websocket/listener/queue-update-transitions.test.ts",
      "test:explicit queue removal emits cancellation rather than dequeue",
    ],
    [
      "src/websocket/listener/turn-terminal-protocol.test.ts",
      "test:finishListenerTurn emits exactly one correlated terminal event",
    ],
  ],
  abort: [
    [
      "src/websocket/listener/protocol-ergonomics.test.ts",
      "test:abort_message sends an ack response when request_id is provided",
    ],
    [
      "src/websocket/listener/turn-terminal-protocol.test.ts",
      "test:finishListenerTurn emits exactly one correlated terminal event",
    ],
  ],
  disconnect: [
    [
      "src/websocket/listener/connection-state-sync.test.ts",
      "test:keeps attached App Server connections from bypassing the startup barrier",
    ],
    [
      "src/websocket/listener/turn-terminal-protocol.test.ts",
      "test:finishListenerTurn emits exactly one correlated terminal event",
    ],
  ],
  "stale-lease": [
    [
      "src/websocket/listener/recovery-lease.test.ts",
      "test:stale recovered tool execution emits nothing into a replacement run",
    ],
    [
      "src/websocket/listener/turn-terminal-protocol.test.ts",
      "test:finishListenerTurn emits exactly one correlated terminal event",
    ],
  ],
  retry: [
    [
      "src/websocket/listener/turn-terminal-protocol.test.ts",
      "test:finishListenerTurn emits exactly one correlated terminal event",
    ],
  ],
  idempotency: [
    [
      "src/websocket/listener/message-router.test.ts",
      "test:preserves the acting user on a directly-owned input and deduplicates retries",
    ],
    [
      "src/websocket/listener/turn-terminal-protocol.test.ts",
      "test:finishListenerTurn emits exactly one correlated terminal event",
    ],
  ],
  "crash-recovery": [
    [
      "src/websocket/listener/recovery-lease.test.ts",
      "test:stale recovered tool execution emits nothing into a replacement run",
    ],
    [
      "src/websocket/listener/turn-terminal-protocol.test.ts",
      "test:finishListenerTurn emits exactly one correlated terminal event",
    ],
  ],
  "vertical-slice": [
    ["src/app-server-client.ts", "method:request"],
    ["src/app-server-client.ts", "method:submitInput"],
    ["src/types/protocol_v2.ts", "WsProtocolMessage"],
    ["src/types/runtime-scope.ts", "RuntimeScope"],
    ["src/types/queue-update-protocol.ts", "QueueRemovalTransition"],
    [
      "src/websocket/listener/loop-state-executing-tools.test.ts",
      "test:reuses the approval request message id for tool lifecycle rows",
    ],
    [
      "src/websocket/listener/protocol-outbound.test.ts",
      "test:fans notifications out to subscribers and honors an explicit target",
    ],
    [
      "src/websocket/listener/turn-terminal-protocol.test.ts",
      "test:finishListenerTurn emits exactly one correlated terminal event",
    ],
  ],
});

export const INVARIANT_PROVENANCE_SPECS = Object.freeze([
    [
      INVARIANTS[0],
      "src/websocket/listener/protocol-outbound.ts",
      "emitProtocolV2Message",
    ],
    [
      INVARIANTS[1],
      "src/websocket/listener/message-router.test.ts",
      "test:preserves the acting user on a directly-owned input and deduplicates retries",
    ],
    [
      INVARIANTS[2],
      "src/websocket/listener/loop-state-executing-tools.test.ts",
      "test:reuses the approval request message id for tool lifecycle rows",
    ],
    [
      INVARIANTS[3],
      "src/websocket/listener/turn-terminal-protocol.test.ts",
      "test:finishListenerTurn emits exactly one correlated terminal event",
    ],
    [
      INVARIANTS[4],
      "src/websocket/listener/recovery-lease.test.ts",
      "test:stale recovered tool execution emits nothing into a replacement run",
    ],
    [
      INVARIANTS[5],
      "src/websocket/listener/protocol-outbound.test.ts",
      "test:fans notifications out to subscribers and honors an explicit target",
    ],
]);
