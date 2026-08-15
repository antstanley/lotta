const [source, url, token, mode = "capture"] = process.argv.slice(2);
if (!source || !url || !token) throw new Error("missing bounded harness argument");
if (!["capture", "fail_after_connect", "stay_alive"].includes(mode)) {
  throw new Error("unknown bounded harness mode");
}
const { AppServerClient } = await import(source);
const { default: WebSocket } = await import("ws");
const frames: Array<{ direction: string; wire: unknown }> = [];
const client = new AppServerClient({
  url,
  authToken: token,
  WebSocket,
  requestTimeoutMs: 5_000,
});
client.onSend((wire: unknown) => frames.push({ direction: "client_to_server", wire }));
let terminalResolve!: () => void;
const terminal = new Promise<void>((resolve) => (terminalResolve = resolve));
client.onMessage((wire: { type?: string }) => {
  frames.push({ direction: "server_to_client", wire });
  if (wire.type === "turn_finished") terminalResolve();
});
try {
  await client.connect();
  frames.push({ direction: "lifecycle", wire: { type: "open" } });
  if (mode === "fail_after_connect") {
    throw new Error("fail_after_connect");
  }
  if (mode === "stay_alive") {
    await new Promise<void>(() => {});
  }
  const runtime = {
    agent_id: "00000000-0000-4000-8000-000000000001",
    conversation_id: "00000000-0000-4000-8000-000000000002",
  };
  await client.runtimeStart({ ...runtime, request_id: "request-runtime-1" });
  let interval: ReturnType<typeof setInterval> | undefined;
  let deadline: ReturnType<typeof setTimeout> | undefined;
  try {
    await new Promise<void>((resolve, reject) => {
      const ready = () => {
        const received = frames.filter(
          (frame) => frame.direction === "server_to_client",
        ).length;
        const lastType = (frames[frames.length - 1]?.wire as { type?: string })?.type;
        return received >= 4 && lastType === "update_queue";
      };
      interval = setInterval(() => {
        if (ready()) resolve();
      }, 1);
      deadline = setTimeout(() => reject(new Error("initial snapshot timeout")), 2_000);
      if (ready()) resolve();
    });
  } finally {
    if (interval !== undefined) clearInterval(interval);
    if (deadline !== undefined) clearTimeout(deadline);
  }
  await client.submitInput({
    runtime,
    request_id: "request-slice",
    payload: {
      kind: "create_message",
      messages: [{
        role: "user",
        content: "<sanitized-trace>",
        client_message_id: "client-message-slice",
      }],
    },
  });
  let terminalTimer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      terminal,
      new Promise((_, reject) => {
        terminalTimer = setTimeout(() => reject(new Error("terminal timeout")), 5_000);
      }),
    ]);
  } finally {
    if (terminalTimer !== undefined) clearTimeout(terminalTimer);
  }
  process.stdout.write(JSON.stringify(frames));
} finally {
  client.close();
}
