import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { WebSocket } from "ws";

import { createRelayServer } from "../src/relay.js";

const activeRelays = [];

afterEach(async () => {
  while (activeRelays.length > 0) await activeRelays.pop().stop();
});

async function startRelay() {
  const relay = createRelayServer({
    host: "127.0.0.1",
    port: 0,
    heartbeatIntervalMs: 60_000,
    logger: { info() {}, warn() {} },
  });
  const address = await relay.start();
  activeRelays.push(relay);
  return { relay, url: `ws://127.0.0.1:${address.port}/ws/tunnel` };
}

function connect(url, role, tunnelId = "test-tunnel-0001", credential = "a".repeat(32)) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    socket.once("error", reject);
    socket.once("open", () => {
      socket.send(JSON.stringify({
        version: 1,
        type: "tunnel.hello",
        role,
        tunnel_id: tunnelId,
        credential,
      }));
    });
    socket.once("message", (data) => {
      const ready = JSON.parse(data.toString());
      assert.equal(ready.type, "tunnel.ready");
      resolve(socket);
    });
  });
}

function nextMessage(socket) {
  return new Promise((resolve, reject) => {
    socket.once("error", reject);
    socket.once("message", (data, isBinary) => resolve({ data, isBinary }));
  });
}

test("health endpoint reports relay state", async () => {
  const { url } = await startRelay();
  const healthUrl = url.replace("ws://", "http://").replace("/ws/tunnel", "/healthz");
  const response = await fetch(healthUrl);
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), {
    status: "ok",
    tunnels: 0,
    desktop_connections: 0,
    mobile_connections: 0,
  });
});

test("forwards text frames between desktop and mobile", async () => {
  const { url } = await startRelay();
  const desktop = await connect(url, "desktop");
  const desktopPeerOnline = nextMessage(desktop);
  const mobile = await connect(url, "mobile");
  assert.equal(JSON.parse((await desktopPeerOnline).data.toString()).type, "tunnel.peer_online");

  const received = nextMessage(desktop);
  mobile.send(JSON.stringify({ kind: "request", id: "request-1" }));
  assert.deepEqual(JSON.parse((await received).data.toString()), {
    kind: "request",
    id: "request-1",
  });

  desktop.close();
  mobile.close();
});

test("rejects a mismatched credential", async () => {
  const { url } = await startRelay();
  const desktop = await connect(url, "desktop");
  const mobile = new WebSocket(url);

  const closeCode = await new Promise((resolve, reject) => {
    mobile.once("error", reject);
    mobile.once("open", () => {
      mobile.send(JSON.stringify({
        version: 1,
        type: "tunnel.hello",
        role: "mobile",
        tunnel_id: "test-tunnel-0001",
        credential: "b".repeat(32),
      }));
    });
    mobile.once("close", (code) => resolve(code));
  });

  assert.equal(closeCode, 4403);
  desktop.close();
});

test("keeps different tunnel ids isolated", async () => {
  const { url } = await startRelay();
  const firstDesktop = await connect(url, "desktop", "test-tunnel-0001");
  const secondDesktop = await connect(url, "desktop", "test-tunnel-0002");
  const firstPeerOnline = nextMessage(firstDesktop);
  const firstMobile = await connect(url, "mobile", "test-tunnel-0001");
  await firstPeerOnline;

  let leaked = false;
  secondDesktop.once("message", () => {
    leaked = true;
  });
  firstMobile.send("only-first-tunnel");
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(leaked, false);

  firstDesktop.close();
  secondDesktop.close();
  firstMobile.close();
});
