# Plan
This codebase is to replace the PHP-based webservices and background scripts for
- magnustools (`~/php/magnustools`, `https://magnustools.toolforge.org/`)
- magnus-toolserver (`~/php/magnus-toolserver`, `https://magnus-toolserver.toolforge.org`)
with a single Rust binary, and accompanying HTML/JS/CSS. It should use `axum`, `wikimisc` (especially the exposed `toolforge`), and other Rust codebases from `~/rust`, where applicable.
The URLs, parameters, etc should not change (except `magnus-toolserver` would be redirects to `magnustools` later). Some files are just static and should continue to serve as such (eg the HTML/JS/CSS from `public_html/resources`).
Many PHP scripts are degraded or broken (especially in `magnus-toolserver`), so try to understand the aim of the respective script, and fix it while translating to Rust.
All web-facing functionality should have a common UI/UX theme, minimalist, sleek, sublime, but always putting usability first. I want a unified look-and-feel for all tools.
`magnustools` should be done first, `magnus-toolserver` should be added in the next phase.

## Decisions
- **Buggregator keeps its current Vue UI** (served as static files); only its PHP backend is ported. It is excluded from the unified theme for now.
- Obsolete/broken pages are dropped or redirected (see endpoint table), not ported faithfully.
- **OAuth**: keep the existing OAuth 1.0a consumers (`oauth.ini`, `buggregator_oauth.ini`); port the flow from `~/rust/mixnmatch_rs/src/auth`. Users re-login once after cutover.
- **Config**: one config file on NFS (like `harvesttemplates`), pointing at `replica.my.cnf`, the OAuth ini files and `flickr_key.txt`. Webservice runs with NFS mounted.
- **Telemetry**: make the payload types in `wikimisc` public and `Deserialize`, so client and ingest share one definition.
- **English only** for the new server-rendered pages (no tooltranslate).

## Scope boundaries
- The PHP repo stays checked out on Toolforge: `public_html/php/` is a library `require_once`'d by other tools (and a Composer package), and `public_html/resources/` is served via tools-static straight from NFS. Rust replaces the **webservice and the cron jobs** only.
- Cutover is all-or-nothing (one webservice per tool), so every kept URL needs a Rust route, static file, or redirect before switching.

## Architecture
- One binary, two modes: `magnustools serve` and `magnustools job <name>` (Toolforge jobs run the same build-service image).
- `harvesttemplates` is the structural template: axum 0.8, `toolforge::pool::WikiPool` (`connect_for_tables` routes Commons link tables to x4), `ToolsDbPool`, `Procfile` + `build.sh`.
- `wikimisc` (git dep): `server`, `logging`, `telemetry`, `database`, `http`, `jsonp`, `escape`, `site-matrix`.
- **Parse, don't validate**: typed extractors (`Wiki` from language/project via an allowlist — no SSRF; `JsonpCallback`; `Qid`; `PageTitle`). Bad input is a 400, never a panic.
- **askama** templates (compile-time checked, auto-escaped — no XSS), one base layout, one small hand-written CSS (light/dark, no framework). Server-rendered forms; JS only where it adds value.
- `main.html` and `tool_status.html` are rendered from the DB (cached), not written to disk by cron.
- Static files from the existing `public_html` via an **extension allowlist** (never serve `.php`, `.pl`, `.pm`, `.inc`, `.svn/`, dotfiles).
- Layout: `src/{main,config,state,params,layout}.rs`, `src/tools/<endpoint>.rs`, `src/buggregator/{api,ingest,dashboard,update}.rs`, `src/oauth/`, `src/jobs/`, `templates/`, `static/`.
- Schema reference: `sql/buggregator.sql` (live `s51079__buggregator`, structure only).

## Milestones (phase 1: magnustools)
| # | Content |
|---|---|
| M0 ✓ | Skeleton: CLI, config, layout/theme, static serving (allowlist), retired URLs, health, telemetry, deploy files. DB pools and typed params come with their first users (M1/M2). |
| M0b ✓ | `wikimisc`: `telemetry-wire` feature with the shared push format. |
| M1 ✓ | Stateless: html2wiki, tab2wiki, spacer, cas, get_article_intro, authority_control, flickrfree, commonsapi (now API-based), prepbio, wiki2playlist (category mode via API), wp_data_url (drop webp, cap fetches, sandboxed) |
| M2 ✓ | Replica DB: get_distinct_authors, quick_counter, herding_sheep, metronom, randomarticle, random_image_commons_subcat, unused_files; fix transparent (actor/linktarget) and uploader_distribution (direct DB intersection instead of dead quick-intersection) |
| M3 ✓ | `logger.php` (exact responses, JSONP fixed) and the main page from DB |
| M4 ✓ | Buggregator backend: `api.php` (all actions; writes POST-only with CSRF check; no SQL in output), `ingest.php` (exact contract, tested against the `wikimisc` client), OAuth 1.0a |
| M5 ✓ | `oauth_uploader.php` (`site` allowlist; downloads only from the public internet) |
| M6 ✓ | Jobs: buggregator update (with Toolhub), webstatus, dashboard maintenance, monitor (mail via Toolforge SMTP); `jobs.yaml`. toolinfo_buggregator.json is served live. |
| Cutover ✓ | 2026-10-23: webservice and jobs switched to Rust; smoke test 23/23. Tools moved to `/<name>/`, `.php` redirects. Rollback: `toolforge webservice stop && toolforge webservice -r 4 php8.4 start`, PHP jobs from `tjs`. |

Phase 2: triage the ~93 magnus-toolserver scripts into port / redirect / drop, then port.

## Endpoint dispositions
Port (M1–M5): all endpoints listed in the milestones.

Static (served as-is): `index.html`, `main.js`, `toolinfo.json`, `toolinfo_buggregator.json` (until M6 generates it), `js_toolinfo.json`, `wdsearch.js`, `wd_useful.js`, `whatelse.js`, `ke.html`, `wikisoba.html`, `autolinker_demo.html`, `can_i_haz_files.html`, `missing_webtools.html`, `mashwiki/`, `pipeline/`, `static_data/`, `resources/`, `buggregator/` frontend, `WikiShootMe.dmg`, `test.kml`.

Drop / redirect:
| Path | Action | Why |
|---|---|---|
| `multistatus.html`, `toolstatus.html` | redirect to `/` | Broken, Toolserver-era |
| `hay2.html` | redirect to `https://hay.toolforge.org/` | Broken |
| `legacy.php` | 410 | Library, not an endpoint |
| `update_article_counter.php` | 410 | Edit-form trick no longer works |
| `file_reuse/` | 410 | Broken (jQuery 404, needs jQuery < 3) |
| `autotools.php` | 410 | Scrapes dead `tools.wmflabs.org` index |
| `last_tools.php` | 410 | Shells out to list `/data/project` |
| `record_ip.php` | 410 | Unauthenticated file append |
| `test1.php`, `test2.html`, `test3.html`, `test.file`, `csstest.html` | 410 | Test leftovers |
| `flommons.pl` | 404 | Perl, already 403 (not an allowlisted file type) |

To check during M2: `cross_cats.php` (OmniScan) and `prep_art.php` — both untracked in git but live.
