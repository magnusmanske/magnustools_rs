# magnustools_rs

Rust replacement for the web service and background jobs of
[magnustools](https://magnustools.toolforge.org/) on Toolforge. URLs and
parameters stay as they were; see `PLAN.md` for scope and progress.

## Run locally

```bash
cp config.json.template config.json   # point public_html at a magnustools checkout
cargo run -- --port 8000
```

Database tools need the tool's `replica.my.cnf` next to `config.json` (or
wherever `replica_credentials` points), with `local='true'` and a
`[tunnels]` section giving each tunnel a local port (see the `toolforge`
crate). Then open the tunnels with:

```bash
TOOL_DATA_DIR=$PWD toolforge-tunnel
```

Static files come from the PHP tool's `public_html`; only allowlisted file
types are served (`src/web/static_files.rs`). Pages share one layout
(`templates/base.html`) and stylesheet (`assets/style.css`).

## Deploy (Toolforge)

`./build.sh` builds the image. The webservice reads
`$TOOL_DATA_DIR/magnustools_rs.json` (see `Procfile` and
`config.json.template`) and needs NFS mounted for `public_html`. The jobs
are in `jobs.yaml`; the same binary runs them (`magnustools_rs --help`).

For local work on anything that writes to the tool database, point
`tool_db.url` at a local copy (`mysql://user@localhost/db?socket=…`);
`sql/buggregator.sql` has the schema.

## License

GPL-2.0-or-later
