Log Search
==========

Quick start:

1. Edit config.toml and set the log files you want to search.
2. Start the app:

   ./start.sh

3. Open the address configured in config.toml.
   The default is:

   http://127.0.0.1:12457

Commands:

  ./log-search --config config.toml rebuild-index
      Rebuild the search index from configured logs.

  ./log-search --config config.toml clear-index
      Clear the local search index.

Install as a systemd service:

  sudo cp -r . /opt/log-search
  sudo cp log-search.service /etc/systemd/system/log-search.service
  sudo systemctl daemon-reload
  sudo systemctl enable --now log-search

The app watches configured log files and updates the index while it runs.
