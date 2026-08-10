import { createHash, randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { WebSocket, WebSocketServer } from "ws";

const DEFAULT_MAX_PAYLOAD_BYTES = 1024 * 1024;
const DEFAULT_MAX_BUFFERED_BYTES = 4 * 1024 * 1024;
const DEFAULT_HANDSHAKE_TIMEOUT_MS = 10_000;
const DEFAULT_HEARTBEAT_INTERVAL_MS = 25_000;
const TUNNEL_ID_PATTERN = /^[A-Za-z0-9_-]{16,128}$/;

function jsonResponse(response, statusCode, payload) {
  const body = JSON.stringify(payload);
  response.writeHead(statusCode, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(body),
    "cache-control": "no-store",
  });
  response.end(body);
}

function sendJson(socket, payload) {
  if (socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify(payload));
  }
}

function credentialHash(credential) {
  return createHash("sha256").update(credential, "utf8").digest("hex");
}

export function createRelayServer(options = {}) {
  const host = options.host ?? "0.0.0.0";
  const port = options.port ?? 18_080;
  const maxPayloadBytes = options.maxPayloadBytes ?? DEFAULT_MAX_PAYLOAD_BYTES;
  const maxBufferedBytes = options.maxBufferedBytes ?? DEFAULT_MAX_BUFFERED_BYTES;
  const handshakeTimeoutMs = options.handshakeTimeoutMs ?? DEFAULT_HANDSHAKE_TIMEOUT_MS;
  const heartbeatIntervalMs = options.heartbeatIntervalMs ?? DEFAULT_HEARTBEAT_INTERVAL_MS;
  const logger = options.logger ?? console;
  const tunnels = new Map();

  const httpServer = createServer((request, response) => {
    if (request.method === "GET" && request.url === "/healthz") {
      let desktopConnections = 0;
      let mobileConnections = 0;
      for (const tunnel of tunnels.values()) {
        if (tunnel.desktop?.readyState === WebSocket.OPEN) desktopConnections += 1;
        for (const mobile of tunnel.mobiles.values()) {
          if (mobile.readyState === WebSocket.OPEN) mobileConnections += 1;
        }
      }
      jsonResponse(response, 200, {
        status: "ok",
        tunnels: tunnels.size,
        desktop_connections: desktopConnections,
        mobile_connections: mobileConnections,
      });
      return;
    }

    jsonResponse(response, 404, { error: "not_found" });
  });

  const websocketServer = new WebSocketServer({
    noServer: true,
    maxPayload: maxPayloadBytes,
    perMessageDeflate: false,
  });

  httpServer.on("upgrade", (request, socket, head) => {
    const requestUrl = new URL(request.url ?? "/", "http://relay.local");
    if (requestUrl.pathname !== "/ws/tunnel" && requestUrl.pathname !== "/") {
      socket.write("HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n");
      socket.destroy();
      return;
    }
    websocketServer.handleUpgrade(request, socket, head, (websocket) => {
      websocketServer.emit("connection", websocket, request);
    });
  });

  websocketServer.on("connection", (socket, request) => {
    socket.isAlive = true;
    socket.tunnelId = null;
    socket.role = null;
    socket.connectionId = randomUUID();
    socket.remoteAddress = request.headers["x-forwarded-for"]?.split(",")[0]?.trim()
      ?? request.socket.remoteAddress
      ?? "unknown";

    socket.on("pong", () => {
      socket.isAlive = true;
    });

    const handshakeTimer = setTimeout(() => {
      if (!socket.tunnelId) socket.close(4408, "handshake timeout");
    }, handshakeTimeoutMs);

    socket.once("message", (data, isBinary) => {
      clearTimeout(handshakeTimer);
      if (isBinary) {
        socket.close(4400, "text handshake required");
        return;
      }

      let hello;
      try {
        hello = JSON.parse(data.toString("utf8"));
      } catch {
        socket.close(4400, "invalid handshake json");
        return;
      }

      const role = hello?.role;
      const tunnelId = hello?.tunnel_id;
      const credential = hello?.credential;
      if (
        hello?.version !== 1
        || hello?.type !== "tunnel.hello"
        || (role !== "desktop" && role !== "mobile")
        || typeof tunnelId !== "string"
        || !TUNNEL_ID_PATTERN.test(tunnelId)
        || typeof credential !== "string"
        || credential.length < 32
        || credential.length > 512
      ) {
        socket.close(4400, "invalid handshake");
        return;
      }

      const hash = credentialHash(credential);
      let tunnel = tunnels.get(tunnelId);
      if (!tunnel) {
        tunnel = {
          credentialHash: hash,
          desktop: null,
          mobiles: new Map(),
          requestRoutes: new Map(),
        };
        tunnels.set(tunnelId, tunnel);
      } else if (tunnel.credentialHash !== hash) {
        socket.close(4403, "invalid credential");
        return;
      }

      if (role === "desktop") {
        const previous = tunnel.desktop;
        if (previous && previous !== socket && previous.readyState !== WebSocket.CLOSED) {
          previous.close(4409, "connection replaced");
        }
        tunnel.desktop = socket;
      } else {
        tunnel.mobiles.set(socket.connectionId, socket);
      }

      socket.tunnelId = tunnelId;
      socket.role = role;
      const peerOnline = role === "desktop"
        ? [...tunnel.mobiles.values()].some((mobile) => mobile.readyState === WebSocket.OPEN)
        : tunnel.desktop?.readyState === WebSocket.OPEN;

      sendJson(socket, {
        version: 1,
        type: "tunnel.ready",
        connection_id: socket.connectionId,
        peer_online: peerOnline,
      });
      if (peerOnline) {
        if (role === "desktop") {
          for (const mobile of tunnel.mobiles.values()) {
            sendJson(mobile, { version: 1, type: "tunnel.peer_online", role });
          }
        } else {
          sendJson(tunnel.desktop, {
            version: 1,
            type: "tunnel.peer_online",
            role,
            mobile_count: tunnel.mobiles.size,
          });
        }
      }

      logger.info?.("tunnel connection ready", {
        tunnel_id: tunnelId,
        role,
        connection_id: socket.connectionId,
        remote_address: socket.remoteAddress,
      });

      socket.on("message", (payload, payloadIsBinary) => {
        socket.isAlive = true;
        const current = tunnels.get(tunnelId);
        if (!current) return;

        if (role === "mobile") {
          const target = current.desktop;
          if (!target || target.readyState !== WebSocket.OPEN) {
            sendJson(socket, { version: 1, type: "tunnel.peer_offline", role: "desktop" });
            return;
          }
          if (target.bufferedAmount > maxBufferedBytes) {
            target.close(1013, "slow consumer");
            sendJson(socket, { version: 1, type: "tunnel.peer_offline", role: "desktop" });
            return;
          }
          if (!payloadIsBinary) {
            try {
              const message = JSON.parse(payload.toString("utf8"));
              if (message?.kind === "request" && typeof message.id === "string") {
                current.requestRoutes.set(message.id, socket.connectionId);
              }
            } catch {
              // 非 JSON 文本仍按透明隧道转发。
            }
          }
          target.send(payload, { binary: payloadIsBinary });
          return;
        }

        let targets = [];
        let routedResponse = false;
        if (!payloadIsBinary) {
          try {
            const message = JSON.parse(payload.toString("utf8"));
            if (message?.kind === "response" && typeof message.id === "string") {
              routedResponse = true;
              const connectionId = current.requestRoutes.get(message.id);
              current.requestRoutes.delete(message.id);
              const mobile = connectionId ? current.mobiles.get(connectionId) : null;
              if (mobile?.readyState === WebSocket.OPEN) targets = [mobile];
            }
          } catch {
            // 非 JSON 文本广播给当前通道内的所有移动设备。
          }
        }
        if (!routedResponse) {
          targets = [...current.mobiles.values()].filter((mobile) => mobile.readyState === WebSocket.OPEN);
        }
        if (targets.length === 0) {
          if (current.mobiles.size === 0) {
            sendJson(socket, { version: 1, type: "tunnel.peer_offline", role: "mobile" });
          }
          return;
        }
        for (const target of targets) {
          if (target.bufferedAmount > maxBufferedBytes) {
            target.close(1013, "slow consumer");
            continue;
          }
          target.send(payload, { binary: payloadIsBinary });
        }
      });
    });

    socket.on("error", (error) => {
      logger.warn?.("tunnel websocket error", {
        connection_id: socket.connectionId,
        message: error.message,
      });
    });

    socket.on("close", () => {
      clearTimeout(handshakeTimer);
      const tunnelId = socket.tunnelId;
      const role = socket.role;
      if (!tunnelId || !role) return;
      const tunnel = tunnels.get(tunnelId);
      if (!tunnel) return;

      if (role === "desktop") {
        if (tunnel.desktop !== socket) return;
        tunnel.desktop = null;
        tunnel.requestRoutes.clear();
        for (const mobile of tunnel.mobiles.values()) {
          sendJson(mobile, { version: 1, type: "tunnel.peer_offline", role });
        }
      } else {
        if (tunnel.mobiles.get(socket.connectionId) !== socket) return;
        tunnel.mobiles.delete(socket.connectionId);
        for (const [requestId, connectionId] of tunnel.requestRoutes.entries()) {
          if (connectionId === socket.connectionId) tunnel.requestRoutes.delete(requestId);
        }
        if (tunnel.mobiles.size === 0) {
          sendJson(tunnel.desktop, { version: 1, type: "tunnel.peer_offline", role });
        }
      }
      if (!tunnel.desktop && tunnel.mobiles.size === 0) tunnels.delete(tunnelId);
      logger.info?.("tunnel connection closed", {
        tunnel_id: tunnelId,
        role,
        connection_id: socket.connectionId,
      });
    });
  });

  const heartbeatTimer = setInterval(() => {
    for (const socket of websocketServer.clients) {
      if (!socket.isAlive) {
        socket.terminate();
        continue;
      }
      socket.isAlive = false;
      socket.ping();
    }
  }, heartbeatIntervalMs);
  heartbeatTimer.unref();

  async function start() {
    await new Promise((resolve, reject) => {
      httpServer.once("error", reject);
      httpServer.listen(port, host, () => {
        httpServer.off("error", reject);
        resolve();
      });
    });
    const address = httpServer.address();
    logger.info?.("Panes Tunnel Relay listening", { address });
    return address;
  }

  async function stop() {
    clearInterval(heartbeatTimer);
    for (const socket of websocketServer.clients) socket.close(1001, "server shutdown");
    await new Promise((resolve) => websocketServer.close(() => resolve()));
    if (httpServer.listening) {
      await new Promise((resolve, reject) => {
        httpServer.close((error) => (error ? reject(error) : resolve()));
      });
    }
  }

  return { httpServer, websocketServer, tunnels, start, stop };
}
