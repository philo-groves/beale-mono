import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { test } from "node:test";
import { BrowserCdpSession, createBrowserTools, managedToolPluginId } from "../packages/research-agent/dist/index.js";

const requireFromServer = createRequire(new URL("../app-server/package.json", import.meta.url));
const { WebSocketServer } = requireFromServer("ws");

test("browser tools expose arbitrary CDP commands, flattened sessions, events, and cleanup", async () => {
  const server = createServer((request, response) => {
    response.setHeader("content-type", "application/json");
    const address = server.address();
    const debuggerUrl = `ws://127.0.0.1:${address.port}/devtools/browser/example`;
    response.end(JSON.stringify(request.url === "/json/list"
      ? [{ id: "page-example", type: "page", webSocketDebuggerUrl: debuggerUrl }]
      : { webSocketDebuggerUrl: debuggerUrl }));
  });
  const sockets = new WebSocketServer({ noServer: true });
  server.on("upgrade", (request, socket, head) => sockets.handleUpgrade(request, socket, head, (websocket) => {
    websocket.on("message", (raw) => {
      const message = JSON.parse(String(raw));
      if (message.method === "Example.fail") {
        websocket.send(JSON.stringify({ id: message.id, error: { code: -32601, message: "Unknown method" } }));
        return;
      }
      websocket.send(JSON.stringify({ id: message.id, result: { method: message.method, params: message.params }, ...(message.sessionId ? { sessionId: message.sessionId } : {}) }));
      websocket.send(JSON.stringify({ method: "Runtime.consoleAPICalled", params: { type: "log" }, ...(message.sessionId ? { sessionId: message.sessionId } : {}) }));
    });
  }));
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const endpoint = `http://127.0.0.1:${server.address().port}`;
  const session = new BrowserCdpSession();
  const tools = createBrowserTools(session);
  const call = async (name, input) => {
    const tool = tools.find((candidate) => candidate.descriptor.name === name);
    return tool.execute({ id: `example-${name}`, toolName: name, actionClass: "experiment", input });
  };
  try {
    assert.equal(tools.length, 5);
    assert.ok(tools.every((tool) => managedToolPluginId(tool.descriptor.name) === "beale-browser"));
    const targets = await call("browser.targets", { endpoint });
    assert.equal(targets.output.targets[0].id, "page-example");
    assert.equal(targets.output.targets[0].webSocketDebuggerUrl, undefined);
    const connected = await call("browser.connect", { endpoint, targetId: "page-example" });
    const connectionId = connected.output.connectionId;
    const command = await call("browser.command", { connectionId, method: "Runtime.evaluate", params: { expression: "1 + 1" }, sessionId: "session-example" });
    assert.deepEqual(command.output, { result: { method: "Runtime.evaluate", params: { expression: "1 + 1" } }, sessionId: "session-example" });
    const protocolError = await call("browser.command", { connectionId, method: "Example.fail" });
    assert.equal(protocolError.status, "error");
    assert.deepEqual(protocolError.output.error, { code: -32601, message: "Unknown method" });
    const events = await call("browser.events", { connectionId });
    assert.equal(events.output.events[0].method, "Runtime.consoleAPICalled");
    assert.equal(events.output.events[0].sessionId, "session-example");
    assert.equal((await call("browser.disconnect", { connectionId })).status, "complete");
    assert.equal((await call("browser.command", { connectionId, method: "Runtime.enable" })).status, "error");
    const direct = await call("browser.connect", { endpoint: `ws://127.0.0.1:${server.address().port}/devtools/browser/example` });
    assert.equal(direct.status, "complete");
    await session.cleanup();
    assert.equal((await call("browser.events", { connectionId: direct.output.connectionId })).status, "error");
  } finally {
    await session.cleanup();
    for (const client of sockets.clients) client.terminate();
    await new Promise((resolve) => sockets.close(resolve));
    await new Promise((resolve) => server.close(resolve));
  }
});
