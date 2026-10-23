# magnustools_rs

Rust replacement for the web service and background jobs of
[magnustools](https://magnustools.toolforge.org/) on Toolforge. URLs and
parameters stay as they were; see `PLAN.md` for scope and progress.

## Run locally

```bash
cp config.json.template config.json   # point public_html at a magnustools checkout
cargo run -- --port 8000
```

Static files come from the PHP tool's `public_html`; only allowlisted file
types are served (`src/web/static_files.rs`). Pages share one layout
(`templates/base.html`) and stylesheet (`assets/style.css`).

## Deploy (Toolforge)

`./build.sh` builds the image. The webservice reads
`$TOOL_DATA_DIR/magnustools_rs.json` (see `Procfile`) and needs NFS mounted
for `public_html`.

## License

GPL-2.0-or-later
