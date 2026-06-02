# Tail 实时查看

Tail 用来实时查看正在写入的普通日志文件，效果类似 Linux 命令：

```bash
tail -f app.log
```

打开 Tail 后，页面会先加载日志文件末尾的一段内容，然后通过 SSE 持续接收新增日志行。

## 页面用法

1. 点击状态行里的 `Show watched`。
2. 在 watched 文件列表里找到要查看的日志。
3. 只有 `hot` 且状态为 `ready` 的文件可以点击 `Tail`。
4. 点击 `Tail` 后会打开一个放大的实时查看弹框。

弹框里可以：

- 查看初始加载的末尾日志行。
- 持续查看新增日志行。
- `Pause`：暂停实时连接。
- `Resume`：从上次收到的位置继续连接。
- `Close`：关闭 Tail 弹框并断开连接。

## 初始行数

默认初始行数是 `10`，和 Linux `tail -f file.log` 一致。

可以在 watched 面板顶部的 `Initial lines` 下拉框里选择：

```text
10 / 50 / 100 / 200 / 500 / 1000
```

这个设置只影响下一次点击 `Tail` 打开的连接，不会打断当前正在查看的 Tail。

## 后端接口

Tail 使用 SSE 接口：

```http
GET /api/tail?fileId=app&lines=10
```

参数：

- `fileId`：日志文件 ID，来自 watched 文件列表。
- `lines`：首次打开时加载末尾多少行。不传时默认 `10`。
- `offset`：续连时从哪个字节位置继续读取。
- `nextLineNo`：续连时下一行的行号。

前端暂停后再恢复时，会带上最后收到的 `offset` 和 `nextLineNo`，避免从头重新加载。

## SSE 事件

服务端推送 `tail` 事件：

```text
event: tail
data: {"path":"/var/log/app.log","offset":12345,"nextLineNo":956,"lines":[...]}
```

字段：

- `path`：日志路径。
- `offset`：当前已经读取到的字节位置。
- `nextLineNo`：下一次新增行的行号。
- `lines`：本次返回的日志行。

空闲时不会持续推空数据，连接通过 SSE keep-alive 保持。

## 限制

- Tail 只支持 `hot` 普通日志文件。
- 压缩文件（`gzip`、`zstd`、`bzip2`、`xz`）不支持 Tail。
- 文件不存在或不是 `hot` 类型时，后端会拒绝请求。
- 当前实现按 offset 读取新增内容；如果日志文件被截断或轮转，后续可以扩展为自动发送 reset 事件并重新加载。
