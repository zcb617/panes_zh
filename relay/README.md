# Panes Tunnel Relay

第一版 Panes 手机远程访问的通用 WebSocket 隧道服务。

## 本地运行

```powershell
npm install
npm test
npm start
```

默认监听 `0.0.0.0:18080`，健康检查地址为 `http://127.0.0.1:18080/healthz`。

## Docker

```powershell
docker compose build
docker compose up -d
docker compose ps
```

Compose 只把容器端口发布到宿主机 `127.0.0.1:18080`。公网连接必须经过 Nginx：

```text
wss://panes.jxrjkf.cn/ws/tunnel
```

## 部署

将整个 `relay` 目录复制到服务器后执行：

```bash
docker compose up -d --build
docker compose ps
curl http://127.0.0.1:18080/healthz
```

Nginx 的 `/ws/tunnel` 应反向代理到 `http://127.0.0.1:18080`。

服务器部署完成后，下面两个检查都应成功：

```bash
curl http://127.0.0.1:18080/healthz
curl https://panes.jxrjkf.cn/healthz
```

如果 Nginx 只对外开放 `/ws/tunnel` 而没有转发 `/healthz`，第二条可以跳过；手机和桌面实际使用的仍然是 `wss://panes.jxrjkf.cn/ws/tunnel`。
