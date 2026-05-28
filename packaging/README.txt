Log Search
==========

快速开始：

1. 编辑 config.toml，配置要搜索的日志文件。
2. 后台启动服务：

   ./start.sh

3. 打开浏览器访问：

   http://127.0.0.1:12457

常用命令：

  ./start.sh
      后台启动服务。

  ./start.sh /path/to/config.toml
      使用指定配置文件后台启动服务。

  CONFIG_FILE=/path/to/config.toml ./start.sh
      也可以通过环境变量指定配置文件。

  ./status.sh
      查看服务是否正在运行。

  ./stop.sh
      停止服务。
      如果 run/log-search.pid 丢失，脚本会尝试停止当前目录下 log-search 二进制启动的进程。
      Linux 上会优先通过进程工作目录匹配，因此也支持 ./log-search 形式启动的进程。

  tail -f logs/log-search.log
      查看运行日志。

  ./log-search --config config.toml rebuild-index
      重新构建搜索索引。

  ./log-search --config config.toml clear-index
      清空本地搜索索引。

安装为 systemd 服务：

  sudo cp -r . /opt/log-search
  sudo cp log-search.service /etc/systemd/system/log-search.service
  sudo systemctl daemon-reload
  sudo systemctl enable --now log-search

  使用 systemd 启动后，请用下面的命令停止服务：

  sudo systemctl stop log-search

服务运行时会监听配置的日志文件，并自动更新索引。
