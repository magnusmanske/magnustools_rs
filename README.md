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
| `/artniw/` | Pages without interwiki links, optionally inside a category tree and among pages using a template (formerly joanjoc.toolforge.org) |
| `/authority_control/` | VIAF lookup (JSON/JSONP) |
| `/blockfinder/` | The global and local blocks that apply to an IP address (formerly erwin85.toolforge.org) |
| `/bong/` | One search box over Wikipedia, Commons, Wikinews, Wiktionary, places |
| `/can_i_haz_files/` | A shell script to download all files in a Commons category tree (formerly magnustools.toolforge.org) |
| `/cas/` | Links for a CAS registry number |
| `/commonsapi/` | XML (or JSON) summary of Commons files |
| `/contribs/` | The contributions of several users, interleaved by time (formerly erwin85.toolforge.org) |
| `/coord_gen/` | German Wikipedia's `{{Coordinate}}` from degrees |
| `/filterdone/` | Removes list entries from a wiki page whose linked page already exists (formerly magnus-toolserver.toolforge.org) |
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
| `/newpages/` | Pages created within a time range (formerly erwin85.toolforge.org) |
| `/no_information/` | Files without an `{{Information}}` or similar template (formerly add-information.toolforge.org) |
| `/not-in-the-other-language/` | Wikidata items with a page in one wiki but not another, optionally limited by a category tree or PagePile (formerly not-in-the-other-language.toolforge.org) |
| `/oauth_uploader/` | Uploads to Commons under the user's account |
| `/prepbio/` | Wikitext for a new biography |
| `/quick_counter/` | A user's edits per namespace |
| `/randomarticle/`, `/random_image_commons_subcat/` | A random page from a category tree |
| `/redirector/` | Redirects a Wikipedia may want, from other languages' titles |
| `/related_places/` | The places a page links to, on a map, as KML or GeoJSON |
| `/shortpages/` | Pages shorter than 50 bytes, optionally without templates or with a single author (formerly erwin85.toolforge.org) |
| `/spacer/` | Links for a spacecraft designator |
| `/subpages/` | The subpages of a page, one title per line (formerly erwin85.toolforge.org) |
| `/sugart/` | Articles with many interwikis that are missing a link to one language (formerly joanjoc.toolforge.org) |
| `/talkcatintersect/` | Articles in one category tree whose talk pages are in another category (formerly erwin85.toolforge.org) |
| `/templator/` | A template, such as an infobox, filled in from a form; templates from `Templator/<Language>` on Meta (formerly templator.toolforge.org) |
| `/transparent/` | Find a translator |
| `/unused_files/` | Commons files not used in articles |
| `/uploader_distribution/` | Uploaders of a category tree |
| `/usualsuspects/` | The uploaders of the files in a category (Bad Boys), and files whose description pages have been left alone (Bad Old Ones) (formerly usualsuspects.toolforge.org) |
| `/wiki2playlist/` | M3U playlist from a wiki page or category |
| `/wiki_todo/` | Things to do in a random article, from `Wiki-ToDo/<Language>` on Meta (also at wiki-todo.toolforge.org) |
| `/wp_data_url/` | An article with its images inlined |
| `/xcontribs/` | How a user's edits are spread over the Wikimedia projects (formerly erwin85.toolforge.org) |
| `/buggregator/api/` | Buggregator API; the front-end (`public/buggregator/`) has the issues, the service dashboard, and the usage dashboard (what the logger counted) |
| `/buggregator/ingest.php` | Service dashboard pushes from `wikimisc` telemetry (also `/buggregator/ingest/`; not a redirect, as the client does not follow those) |
| `/toolinfo_buggregator.json` | Toolinfo for tools not on Toolhub yet |

Uses of the tools are counted in the telemetry (`tool_uses_total`, with
`tool_requests_total` for all requests, in `src/tools/mod.rs`): a request
counts if it brings parameters and does not fail, so opening an empty form
does not. The service dashboard's hourly maintenance copies the uses of the
last complete days into the tool database's usage log, as tool `magnustools`
with the tool's name as method (`src/buggregator/dashboard/daily_uses.rs`);
it does the same for wikidata-todo, and for any service that reports
`tool_uses_total`. The logger itself (`/logger/`) is for the other tools.

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

### Database migrations

`sql/` holds the reference schema and the one-off migrations. Back up the tool
database and stop the web service and jobs, then run them in order:

1. `2026-11-09-git-issue-site.sql`, `2026-11-09-wiki-duplicates.sql`
2. `2026-11-10-dedupe-prep.sql`
3. `magnustools_rs --config … buggregator-dedupe` (dry run), then `--apply`
4. `2026-11-10-utf8mb4.sql`, `2026-11-10-null-dates-tools.sql`,
   `2026-11-10-drop-legacy-views.sql`, `2026-11-10-token-hash-ascii.sql`

`buggregator-dedupe` reports duplicate issues, users, texts and links;
`--apply` collapses them and installs the natural keys. Do not run it against a
database that is still being written to.

## Jobs

The same binary runs the background jobs: `magnustools_rs --help` lists them,
and `jobs.yaml` schedules them on Toolforge. Service dashboard administration
is `magnustools_rs dashboard add|rotate|enable|disable|list|reinfer`; the
one-off duplicate cleanup is `magnustools_rs buggregator-dedupe`.

Alerts go to `monitor.mail_to`. The `monitor` job covers tools without a
dashboard service (a blocked bot, a web address that does not answer with
200); `dashboard-alerts` covers the services on the dashboard (no report for
ten minutes, no working web address, crash loops, memory). Each problem is
mailed once, and again each day while it lasts; the dashboard and usage pages
show the failing web addresses as well.

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
