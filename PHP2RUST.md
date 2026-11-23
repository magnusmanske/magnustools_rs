# PHP to Rust: which tools to convert

Triage of the tools on the [main page](https://magnustools.toolforge.org/) that are not Rust yet, by expected gain in efficiency, stability, speed and memory. As of 2026-11-23.

Based on: usage over 30 days to 2026-11-23 (logger uses, dashboard requests), the code (`~/php/<tool>` and `/data/project/<tool>`), live HTTP checks on 2026-11-23, job status on Toolforge, and each tool's live `service.manifest`. The `tool` table now has the magpie rows (`commonshelper`, `geograph2commons`, `url2commons`, `flickr2commons`) as `rust/axum`, and all active PHP rows are `php8.4` (2026-11-23); the live `service.manifest` remains the source of truth.

## Already Rust

PetScan, Mix'n'match, QuickStatements (web and bot, since ~2026-10-21), Listeria (web, since 2026-10-23), geohack, autodesc, wdrc, Harvest Templates, wd-infernal, ac2wd, RustCCI, magnustools, wikidata-todo (`wikidata_todo_rs`, including `sparql_rc`), catfood (since 2026-10-24), missingtopics (part of magnustools since 2026-10-24). add-information (part of magnustools since 2026-10-25). makeref and templator (part of magnustools since 2026-10-26), icommons (since 2026-10-27). BaGLAMa2 is ~95% Rust (`baglama2_rs`; only a thin read API and the add-category action are PHP). Seven of erwin85's tools (blockfinder, contribs, newpages, shortpages, subpages, talkcatintersect, xcontribs) are part of magnustools since 2026-11-06; the rest depended on the removed `mysql_*` functions or were already disabled. joanjoc's sugart and artniw are part of magnustools since 2026-11-07; its raw SQL consoles were not ported. After reviewing the deactivated magnustools and magnus-toolserver PHP pages, can_i_haz_files and filterdone were rescued on 2026-11-07; the rest depended on dead Toolserver databases or services, were libraries or tests, or have newer replacements. usualsuspects (Bad Boys and Bad Old Ones) is part of magnustools since 2026-11-08, and so is not-in-the-other-language; both old subdomains redirect with their parameters. sighting's deep_insight, deep_out_of_sight and random_out_of_sight are part of magnustools since 2026-11-10; its old subdomain redirects with its parameters, and the deactivated outofsight is gone. dnbtools' four web pages (dnb_link_finder, dnb_ratios, dnb_wikisource, map2wp) are part of magnustools since 2026-11-10, with the old SQL fixed for the current schemas; its subdomain redirects with its parameters. The cached word counts still come from the old `ws_dnb_wordcount.tab` data file. most-wanted is part of magnustools since 2026-11-11: the daily `most-wanted-<wiki>` jobs rebuild the flat `.links.red` lists into `/data/project/magnustools/most_wanted/`, and the old subdomain redirects with its parameters. referee is now a Rust endpoint in `wd-infernal` (`/referee/{item}`); its old `referee.toolforge.org` subdomain redirects there (2026-11-13).

PagePile's service is also Rust (`~/rust/pagepile`, plus the `pagepile` client crate); the PHP class and entry points remain for tools that include them. isbn2wiki is part of magnustools since 2026-11-12, with its ISBN lookups in the published `bookmeta` crate; its old subdomain is a Toolforge redirect container. WD-FIST's core logic is Rust in `petscan_rs` (`src/wdfist.rs`, PetScan's `wdf_main` mode); only the standalone `fist.toolforge.org/wdfist` UI/API is still PHP. Catfood's service is Rust; its stale `commons_image_feed` row is now marked retired.

magpie (`~/rust/magpie`) is Rust/axum: commonshelper, url2commons, flickr2commons and geograph2commons moved there on 2026-11-20, with one OAuth login and upload path (the `mediawiki_upload` crate). The four old subdomains are Toolforge redirect containers; the old `.php` addresses redirect to the matching magpie page.

**Status on 2026-11-23:** no new magnustools ports since 2026-11-13. The four upload tools (commonshelper, url2commons, flickr2commons, geograph2commons) moved to the Rust/axum `magpie` service on 2026-11-20; their old subdomains are redirect containers. All PHP tools checked on 2026-11-23 still answer 200: GLAMorous (both `/glamorous/` and `/glamorous.php`), tooltranslate, WikiShootMe, FIST, sourcemd, reasonator, WikiFlix and the low-use tools. WikiShootMe, Reasonator and WikiFlix now run `php8.4`, and the `tool` table's active PHP rows were updated to match on 2026-11-23. Reasonator's buggregator backlog is still 4 open (WIKI 54, 66, 315; Codeberg #40); commonshelper has 15 open (all Codeberg, down from 24), geograph2commons 13, and sourcemd 0 (down from 32). Post-port fixes since 2026-11-13: the most-wanted scan was chunked to stay inside the replica statement limit (2026-11-14), commonsapi's batch image fetch was parallelized (2026-11-22), and missingtopics was reworked (2026-11-23).

Usage figures below are from the 30 days ending 2026-11-23. They are logger uses unless noted; the Rust tools' dashboard uses are counted separately.

## Redirecting an old tool

Do not add a redirect page to the old PHP code. Use Toolforge's
[redirect container](https://wikitech.wikimedia.org/wiki/Tool:Containers#Redirect_container),
which redirects every path and keeps the query string (308, so a POST body
survives too).

As the old tool's account (`become <tool>` on `login.toolforge.org`):

```bash
toolforge envvars create REDIRECT_HOST magnustools.toolforge.org
toolforge envvars create REDIRECT_PATH /<new-tool>/
cat > "$HOME/service.template" <<'EOF'
type: buildservice
buildservice-image: tool-containers/redirect:latest
health-check-path: /healthz
mount: none
cpu: 125m
memory: 128Mi
EOF
toolforge webservice stop
toolforge webservice start
```

`REDIRECT_PATH` without `%s` sends every old path to the same new page; use
`/<new-tool>%s` to keep the old path suffix. `REDIRECT_PROTO` defaults to
`https`. Examples: `isbn2wiki.toolforge.org` →
`https://magnustools.toolforge.org/isbn2wiki/` (2026-11-12), and
`commonshelper.toolforge.org` → `https://magpie.toolforge.org/commonshelper/`
(2026-11-20).

## Finish, don't rewrite

- **PagePile**: done; the `tool` table row is `rust/axum` (checked 2026-11-11). The PHP class stays for tools that include it; the old `cron-0` job still fails and should be removed (see below).
- **WD-FIST**: the core is already Rust. Decide whether to retire/redirect the standalone PHP UI or expose the existing `petscan_rs` module; do not port the algorithm again. Legacy FIST still answers 200 and logged ~175k uses in the 30 days to 2026-11-23 (the logger's `run` count fell sharply on 2026-11-12, while access logs still show 10–50k requests/day), so point its JSON at FIST2 rather than removing the address.
- **WikiShootMe**: the map and search work is in the browser; PHP is OAuth, upload and API pass-through. It now runs php8.4 and had ~14.3k uses/30d; 37 buggregator rows are open (25 Codeberg + 12 WIKI). Fix the PHP in place (timeouts on `file_get_contents`, OAuth token handling) instead of porting.
- **Reasonator**: do not port. V2 (`public_html/v2/`) is client-side JS; the PHP layer only handles OAuth and OpenGraph/Twitter metadata. It now runs php8.4, answers 200, and the buggregator backlog is still 4 open rows (WIKI 54, 66, 315; Codeberg #40). Fix V2/autodesc, not the PHP shell.
- **wikidata-terminator**: ported on 2026-11-13; the old `/api.php` read API is kept, the Vue app is now a server-rendered page, and the daily `terminator-refresh` job rebuilds the candidate index (`src/tools/terminator/`) with 4 GiB. The index moved to `s51079__terminator_p`; the old subdomain redirects with its path (`REDIRECT_PATH=/wikidata-terminator%s`). 7 Codeberg issues remain open.
- **magpie**: the four upload tools are ported; do not port their PHP again. Triage the 51 open Codeberg issues (commonshelper 15, geograph2commons 13, url2commons 1, flickr2commons 22) against the new service.

## Ranking (new PHP ports)

| # | Tool | Use (30 days to 2026-11-23) | Why Rust helps | Effort | Status |
|---|---|---|---|---|---|
| 1 | GLAMorous (`glamtools/public_html/glamorous.php`) | 8.4k | Loads every file name of a category tree (depth up to 20) to count them. Rust: stream and aggregate; reuse `baglama2_rs`'s `global_image_links` and category-tree code. Mostly a memory gain. | M | Still PHP 8.4; 1 open WIKI row; 115 `count()` TypeErrors/30d at `glamorous.php:394`. |
| 2 | tooltranslate API (`tooltranslate/public_html/api.php`) | 463 | Security, not performance: `update_json` writes under a path built from the request tool name; `add_tool` lets any logged-in user create a name with `../`; SQL strings are returned in responses; write modes lack an explicit CSRF token. Port only the write API (or fix in PHP). | S–M | Still PHP 8.4; 10 open issues (9 Codeberg + 1 WIKI); 5 OAuth token errors/30d. |

Ranked by expected gain, not raw traffic. GLAMorous still spends the most memory per use; tooltranslate is a security fix more than a performance port. most-wanted and isbn2wiki were ported on 2026-11-11 and 2026-11-12, so they are no longer ranked; their 30-day logger totals are mostly pre-port traffic. WikiFlix (94k uses/30d) is the largest remaining PHP tool, but it has not been analysed; its error log suggests a cheap fix first (see below).

## Not worth porting

- **Legacy FIST `fist.php`**: still the least stable code: in the 30 days to 2026-11-23 it logged ~27k "max_user_connections" fatals from opening a replica connection per language link, ~40k `commonswiki_p.templatelinks` invalid-view fatals (and some missing `categorylinks`), and ~12k invalid replica names (`zh-min-nan`, `be-x-old`, `zh-yue`, …). It still logged ~175k uses/30d, so point its JSON at FIST2 rather than removing the address. Do not port 3.7k lines.
- **WikiShootMe**: the map work runs in the browser; PHP is OAuth and upload pass-through. It now runs php8.4; fix the remaining PHP (timeouts, OAuth) rather than port.
- **Wikidata Game**: the shared game API (`desc`/`tiles`/`log_action`) is already in `wikidata_todo_rs`, but the PHP `get_candidate` logic and batch jobs remain. Both jobs still fail, and the error log is full of `distributed/demo/primary_sources.php` fetching the retired `tools.wmflabs.org` endpoint (404, then a 30 s timeout). Retire or fix the PHP tool rather than porting it whole; secure `update_game`/`update_all_games` meanwhile.
- **sourcemd** (0 logged uses, ongoing SPARQL failures; 0 open buggregator rows, down from 32): don't port.
- **tooltranslate consumers**: consumers load static JSON from tools-static; only the editor is PHP. The fragile part for other tools is `tt.php` (`file_get_contents` without timeout). The editor API is a small security fix (see above); the rest is not worth porting.
- **GLAMorgan/TreeViews pageview proxy, BaGLAMa2 API**: thin or I/O-bound.
- **url2commons, flickr2commons, commonshelper, geograph2commons**: now in `magpie` (Rust); no longer port targets. See "Already Rust".
- **Other wikidata-todo endpoints** (`dupe_finder`, `beacon`, `true_duplicates`, `user_edits`, `add_name_labels`, `sexer`, `firstnamebasis`, `new_items`, …): several use removed tables or scan everything; retire rather than port. Several of these are already Rust in `wikidata_todo_rs`.
- **WikiFlix**: now php8.4 and ~94k uses/30d, but not analysed (no local checkout); its web and job logs have ~984k `applyDbStatementTimeout: ... getSQL(): Argument #2 ($sql) could not be passed by reference` errors in 30 days (273 MB). Fix that call or triage for a port; do not port blind.
- **Low-use or unmaintained PHP tools** (cradle, drawshield, pauken, picturesque, projector, quest, tabernacle, toolflow, topicmatcher, wlmuk, book2scroll, …): don't port unless they have a maintainer and real users. book2scroll had ~7.8k uses/30d but was not analysed; the logged ones are at or below a few hundred. catnap, file-siblings, filedupes, multidesc and new-page-linker moved to magnustools on 2026-11-13.

## Fix now, Rust or not

Security:

- [x] wikidata-terminator: ported on 2026-11-13; the SQL-injection note was already stale, and the new query builder validates language/type and uses bound SQL parameters. `main_p31` is seeded by the refresh job.
- [ ] Wikidata Game: `distributed/api.php` `update_game`/`update_all_games` are unauthenticated and call every registered game's `desc` endpoint. Require auth or run only from cron. The shared game API is Rust, but the classic game's candidate logic is still PHP; both scheduled jobs still fail.
- [x] tooltranslate: `store_translation`/`add_tool` do require OAuth, but `update_json` is unauthenticated and builds the output path from the tool name. A logged-in user can add a name with `../`; writes are limited to `toolinfo.json`/`<language>.json` but can overwrite static JSON. Responses also include SQL strings. Fix in PHP now (typed names, no SQL in output, CSRF) whether or not it is ported; 10 open issues (9 Codeberg + 1 WIKI) and 5 OAuth token errors/30d.

Failing jobs:

- [x] Listeria `update-wikis`: the job is gone (the old `update-wikis.err` is from 2026-10-20); the continuous `rustbot` job is running (checked 2026-11-23).
- [x] PagePile `cron-0` (`touch_data.sh`, deprecated `php7.4` image) still fails every 5 minutes; `jobs logs` has no current output and the last written `cron-0.err` (2024) shows `touch: setting times of '/data/project/shared/pagepile/...': Permission denied`. The web service is Rust now and this repo's `jobs.yaml` has no pagepile job; remove the old Toolforge job or fix the shared-directory permissions.
- [ ] Wikidata Game `game-maintenance` and `update-distributed-game` still fail; the error log shows `distributed/demo/primary_sources.php` requesting the retired `tools.wmflabs.org` endpoint (404) and 30 s timeouts.
- [ ] FIST: only `commons-random-depicts` and `on-wikidata` are still on the deprecated php7.4 image, and both fail; the other jobs are php8.4. `fc-taxon-media.out` is ~95 MB.
- [x] Mix'n'match: progress resumed; 4 jobs were running and `scroll_rerank` entries were logged on 2026-11-23, so the "no progress since 2026-10-20" note is stale.
- [x] Mix'n'match telemetry: pushes to `/buggregator/ingest.php` had 503/connection errors (10 in `rustbot.err`; last write 2026-11-23 08:47), so the dashboard may miss Mix'n'match uses.
- [x] QuickStatements (Rust bot): still many `Key "labels" not found` in `bot.out`; `bot.err` also shows `Can not parse item(s) ["L…-S…"]` for lexeme statements.

Cheap PHP fixes for the worst errors:

- [x] most-wanted: ported; the `max()` guard is in `hint_categories`, hints/number are capped, and the template-usage query is batched. The 2026-11-14 follow-up chunks `linktarget` ids and filters blue links after grouping, keeping the scan inside the replica statement limit.

- [ ] `ToolforgeCommon.php:705` (`getSPARQLitems` reads `$row->q` whatever the variable): the source of nearly all of the old wikidata-todo error log; the Rust `sparql_rc` stopped the growth, but other PHP tools still include this library (sourcemd still logs `getSPARQLitems` stack traces).
- [ ] FIST `fist.php`: reuse one connection per replica cluster; map renamed wiki names via the site matrix (e.g. `zh-min-nan`, `be-x-old`, `zh-yue`, `roa-tara`, `bat-smg`); stop using the invalid `commonswiki_p.templatelinks` view and missing `categorylinks` table.
- [ ] GLAMorous: reject a non-list `projects` parameter (115 `count()` TypeErrors/30d at `glamorous.php:394`).
- [ ] WikiFlix: fix the `applyDbStatementTimeout` by-reference call into `ToolforgeCommon::getSQL()`; ~984k errors/30d across the web and update logs (273 MB).

Other:

- [x] Listeria: `query_list.php` now answers 404; no Rust route is needed.
- [x] Tool table: `webserver_type` set from the live `service.manifest` for the 18 subdomains checked on 2026-10-23.
- [x] Tool table: usualsuspects and not-in-the-other-language rows updated on 2026-11-08 to `magnustools` / `rust/axum` with the new URLs.
- [x] Tool table: sighting rows updated on 2026-11-10 to `magnustools` / `rust/axum` with the new URLs; outofsight marked as deactivated.
- [x] Tool table: dnbtools rows updated on 2026-11-10 to `magnustools` / `rust/axum` with the new URLs.
- [x] Tool table: pagepile is now `rust/axum` (checked 2026-11-11).
- [x] Tool table: catfood's old `commons_image_feed` row is marked retired (2026-11-13); its main row is `rust/axum`.
- [x] Tool table: `referee` moved to `wd-infernal` / `rust/axum` (2026-11-13); the old subdomain redirects there.
- [x] Tool table: erwin85 and joanjoc redirect rows are now `rust/axum` (2026-11-13).
- [x] Tool table: `baglama2`, `WD-FIST` and `distributed game` notes reflect their Rust parts (2026-11-13).
- [x] Tool table: the magpie rows (commonshelper, geograph2commons, url2commons, flickr2commons) are `rust/axum` with magpie URLs (2026-11-20).
- [x] Tool table: all active PHP rows updated to `php8.4` (2026-11-23): `reasonator`/`geneawiki*`/`widee`, `toolflow`, `wikiflix`, `wikishootme`, and `upload-stats-bot` (`has_webservice` stays 0); see `sql/2026-11-23-tool-php-versions.sql`.
- [x] Reasonator: not a port target; V2 is JS and the PHP shell is OAuth/OpenGraph. The 503 is fixed and the backlog is still down to 4 (2026-11-23).
- [x] The leftover `/tmp/wdt_tail.*` is gone (checked 2026-11-23).
