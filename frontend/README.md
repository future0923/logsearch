# Log Search Frontend

前端使用 React + Vite。

## 本地开发

默认连接本机后端：

```bash
npm install
npm run dev
```

默认代理地址是：

```text
http://127.0.0.1:12457
```

## 切换远程后端

先测试远程后端是否能访问：

```bash
npm run test:remote -- http://192.168.0.10:12457
```

启动前端并连接远程后端：

```bash
npm run dev:remote
```

当前默认远程地址在 `package.json` 里：

```text
http://192.168.0.10:12457
```

临时连接其他后端：

```bash
npm run dev:remote -- http://192.168.0.10:12457
```

也可以使用环境变量：

```bash
VITE_API_PROXY_TARGET=http://192.168.0.10:12457 npm run dev
VITE_API_BASE=http://192.168.0.10:12457 npm run dev
```

如果发布包前后端放在同一个服务里，不需要设置 `VITE_API_BASE`。
