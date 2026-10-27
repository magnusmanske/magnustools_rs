# magnustools_rs

The web service and background jobs of [magnustools](https://magnustools.toolforge.org/),
a [Toolforge](https://toolforge.org/) tool by Magnus Manske: small tools for
Wikimedia projects, the tool list, a usage logger, an OAuth uploader, and the
[Buggregator](https://magnustools.toolforge.org/buggregator/) issue tracker
and service dashboard.

It also hosts what is left of [magnus-toolserver](https://codeberg.org/magnusmanske/magnus-toolserver),
whose addresses all redirect to the same path here.

This replaces the PHP version, [magnustools on Codeberg](https://codeberg.org/magnusmanske/magnustools/).
That repository stays in use for what is not a web service: the PHP library
other tools include, and the shared static files (`public_html/resources/`),
which this service serves from its checkout.

## Tools

Each tool lives at `/<name>/`; its old address, `/<name>.php`, redirects there
(permanently, keeping the query string and, for POST, the body).

| Path | What it does |
|---|---|
| `/` | The list of all tools |
| `/add_information/` | Rewrites a file description into an `{{Information}}` template, for the user to save (formerly add-information.toolforge.org) |
| `/authority_control/` | VIAF lookup (JSON/JSONP) |
| `/bong/` | One search box over Wikipedia, Commons, Wikinews, Wiktionary, places |
| `/cas/` | Links for a CAS registry number |
| `/commonsapi/` | XML (or JSON) summary of Commons files |
| `/coord_gen/` | German Wikipedia's `{{Coordinate}}` from degrees |
| `/flickrfree/` | Latest freely licensed Flickr photos |
| `/forest4trees/` | Articles that may be missing from a category tree |
| `/get_article_intro/` | An article's first paragraph (JSON/JSONP) |
| `/get_distinct_authors/` | Who edited a set of articles |
| `/herding_sheep/` | Recent edits by a group of users |
| `/html2wiki/`, `/tab2wiki/` | HTML or spreadsheet tables to wiki markup |
| `/icommons/` | Commons for small screens: picture of the day, search, files, categories (formerly icommons.toolforge.org) |
| `/logger/` | Counts tool uses (JSON/JSONP) |
| `/makeref/` | A citation template filled in from a form; types and fields from `MakeRef/<Language>` on Meta (formerly makeref.toolforge.org) |
| `/metronom/` | Page views of the articles a user started |
| `/missingtopics/` | Red links of an article or category tree, ranked by demand (formerly missingtopics.toolforge.org) |
| `/misstake/` | Red links on some pages, ranked by how wanted they are |
| `/no_information/` | Files without an `{{Information}}` or similar template (formerly add-information.toolforge.org) |
| `/oauth_uploader/` | Uploads to Commons under the user's account |
| `/prepbio/` | Wikitext for a new biography |
| `/quick_counter/` | A user's edits per namespace |
| `/randomarticle/`, `/random_image_commons_subcat/` | A random page from a category tree |
| `/redirector/` | Redirects a Wikipedia may want, from other languages' titles |
| `/related_places/` | The places a page links to, on a map, as KML or GeoJSON |
| `/spacer/` | Links for a spacecraft designator |
| `/templator/` | A template, such as an infobox, filled in from a form; templates from `Templator/<Language>` on Meta (formerly templator.toolforge.org) |
| `/transparent/` | Find a translator |
| `/unused_files/` | Commons files not used in articles |
| `/uploader_distribution/` | Uploaders of a category tree |
| `/wiki2playlist/` | M3U playlist from a wiki page or category |
| `/wp_data_url/` | An article with its images inlined |
| `/buggregator/api/` | Buggregator API; the front-end (`public/buggregator/`) has the issues, the service dashboard, and the usage dashboard (what the logger counted) |
| `/buggregator/ingest.php` | Service dashboard pushes from `wikimisc` telemetry (also `/buggregator/ingest/`; not a redirect, as the client does not follow those) |
| `/toolinfo_buggregator.json` | Toolinfo for tools not on Toolhub yet |

Uses of the tools are counted per day in the tool database, as tool
`magnustools` with the tool's name as method (`src/tools/logger.rs`): a
request counts if it brings parameters and does not fail, so opening an empty
form does not.

The tools with their own interface keep their files in `public/`, built into
the binary (`src/web/apps.rs`): the Buggregator front-end, and from
magnus-toolserver `thetalkpage/` and `ts2/geneawiki/`, `ts2/geneawiki2/`
(family trees from Wikidata); also the `commcomm/` files that Commons'
`MediaWiki:Commander.js` loads. Changes to them belong here, not in the PHP
checkout, whose copies are no longer served.

So all tools work without the PHP checkout. If `public_html` is configured,
anything else comes from there: the libraries other tools load
(`resources/`), and pages of retired tools, if their type is on an allowlist
(`src/web/static_files.rs`). Retired addresses redirect or answer 410
(`src/web/legacy.rs`).

## Code

- `src/tools/`: one module per tool; `templates/tools/` their pages.
- `public/`: the HTML, JavaScript and CSS of the tools with their own interface.
- `src/buggregator/`: API, ingest, dashboard, and the update job.
- `src/jobs/`: background jobs and admin commands.
- `src/web/`: routing, errors, parameters, static files. `templates/base.html`
  and `assets/style.css` give all pages one look.
- `src/wiki.rs`, `src/mediawiki.rs`, `src/replicas.rs`, `src/tool_db.rs`,
  `src/oauth.rs`: wikis, their API and databases, the tool database, OAuth.

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

## Run locally

```bash
cp config.json.template config.json   # public_html (a magnustools checkout) is optional
cargo run -- --port 8000
```

Database tools need the tool's `replica.my.cnf` next to `config.json` (or
wherever `replica_credentials` points), with `local='true'` and a `[tunnels]`
section giving each tunnel a local port (see the `toolforge` crate). Open the
tunnels with:

```bash
TOOL_DATA_DIR=$PWD toolforge-tunnel
```

For anything that writes to the tool database, point `tool_db.url` at a local
copy (`mysql://user@localhost/db?socket=…`); `sql/buggregator.sql` has the schema.

## Jobs

The same binary runs the background jobs: `magnustools_rs --help` lists them,
and `jobs.yaml` schedules them on Toolforge. Service dashboard administration
is `magnustools_rs dashboard add|rotate|enable|disable|list|reinfer`.

## Deploy

On Toolforge, as the `magnustools` tool:

```bash
./build.sh                                              # build the image from GitHub
toolforge webservice buildservice restart --mount=all   # or start, the first time
toolforge jobs load jobs.yaml
```

The service reads `$TOOL_DATA_DIR/magnustools_rs.json` (see
`config.json.template` and the `Procfile`).

## License

GPL-2.0-or-later
