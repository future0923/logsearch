# Log Search

Log Search is a single-process log search app. The backend serves the UI and API, watches configured log files, and keeps the local index updated.

## User Quick Start

```bash
tar -xzf log-search-0.1.0-linux-x64.tar.gz
cd log-search-0.1.0-linux-x64
vim config.toml
./start.sh
```

Open:

```text
http://127.0.0.1:12457
```

Change `[server].addr` in `config.toml` to expose it on another host or port.
