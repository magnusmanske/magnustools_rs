#!/bin/bash
# Wiki-replica / ToolsDB tunnels for local development: opens the tunnels in
# the [tunnels] section of ./replica.my.cnf (see src/mysql_misc.rs) until
# Ctrl-C. `cargo run` finds the same file (TOOL_DATA_DIR, .cargo/config.toml).
# Mix'n'match only needs ToolsDB, wikidatawiki core and its term store — it
# queries no Commons and no links tables.
cd "$(dirname "$0")" && TOOL_DATA_DIR=. exec toolforge-tunnel
