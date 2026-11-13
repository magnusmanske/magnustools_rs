# PHP to Rust: which tools to convert

Triage of the tools on the [main page](https://magnustools.toolforge.org/) that are not Rust yet, by expected gain in efficiency, stability, speed and memory. As of 2026-11-11.

Based on: usage over 30 days to 2026-11-11 (logger uses, dashboard requests), the code (`~/php/<tool>` and `/data/project/<tool>`), live HTTP checks on 2026-11-11, job status on Toolforge, and each tool's live `service.manifest`. The `tool` table's `webserver_type` is still stale in a few tombstone rows (the old `catfood` `commons_image_feed` row and the erwin85/joanjoc redirect rows), so the live `service.manifest` remains the source of truth.

## Already Rust

PetScan, Mix'n'match, QuickStatements (web and bot, since ~2026-10-21), Listeria (web, since 2026-10-23), geohack, autodesc, wdrc, Harvest Templates, wd-infernal, ac2wd, RustCCI, magnustools, wikidata-todo (`wikidata_todo_rs`, including `sparql_rc`), catfood (since 2026-10-24), missingtopics (part of magnustools since 2026-10-24). add-information (part of magnustools since 2026-10-25). makeref and templator (part of magnustools since 2026-10-26), icommons (since 2026-10-27). BaGLAMa2 is ~95% Rust (`baglama2_rs`; only a thin read API and the add-category action are PHP). Seven of erwin85's tools (blockfinder, contribs, newpages, shortpages, subpages, talkcatintersect, xcontribs) are part of magnustools since 2026-11-06; the rest depended on the removed `mysql_*` functions or were already disabled. joanjoc's sugart and artniw are part of magnustools since 2026-11-07; its raw SQL consoles were not ported. After reviewing the deactivated magnustools and magnus-toolserver PHP pages, can_i_haz_files and filterdone were rescued on 2026-11-07; the rest depended on dead Toolserver databases or services, were libraries or tests, or have newer replacements. usualsuspects (Bad Boys and Bad Old Ones) is part of magnustools since 2026-11-08, and so is not-in-the-other-language; both old subdomains redirect with their parameters. sighting's deep_insight, deep_out_of_sight and random_out_of_sight are part of magnustools since 2026-11-10; its old subdomain redirects with its parameters, and the deactivated outofsight is gone. dnbtools' four web pages (dnb_link_finder, dnb_ratios, dnb_wikisource, map2wp) are part of magnustools since 2026-11-10, with the old SQL fixed for the current schemas; its subdomain redirects with its parameters. The cached word counts still come from the old `ws_dnb_wordcount.tab` data file. most-wanted is part of magnustools since 2026-11-11: the daily `most-wanted-<wiki>` jobs rebuild the flat `.links.red` lists into `/data/project/magnustools/most_wanted/`, and the old subdomain redirects with its parameters.

PagePile's service is also Rust (`~/rust/pagepile`, plus the `pagepile` client crate); the PHP class and entry points remain for tools that include them. isbn2wiki is part of magnustools since 2026-11-12, with its ISBN lookups in the shared `bookmeta` crate. WD-FIST's core logic is Rust in `petscan_rs` (`src/wdfist.rs`, PetScan's `wdf_main` mode); only the standalone `fist.toolforge.org/wdfist` UI/API is still PHP. Catfood's service is Rust; one stale `commons_image_feed` row in the tool table still says PHP.

**Status on 2026-11-11:** the 503 incident is over; the PHP tools checked (`isbn2wiki`, GLAMorous, tooltranslate, WikiShootMe, commonshelper, FIST, sourcemd, geograph2commons, reasonator) all answer 200. The recent ports are `usualsuspects` and `not-in-the-other-language` (2026-11-08), sighting's three pages (2026-11-10), dnbtools' four pages (2026-11-10), and most-wanted (2026-11-11, with 11 daily wiki jobs). PagePile's tool row is now `rust/axum`. Reasonator is not a PHP port target: V2 is client-side JS and the PHP shell is OAuth/OpenGraph metadata; its buggregator backlog is down to 4 open (WIKI 54, 66, 315; Codeberg #40).

Usage figures below are from the 30 days ending 2026-11-11. Those tools are no longer counted as down.

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
mem: 128Mi
EOF
toolforge webservice stop
toolforge webservice start
```

`REDIRECT_PATH` without `%s` sends every old path to the same new page; use
`/<new-tool>%s` to keep the old path suffix. `REDIRECT_PROTO` defaults to
`https`. Example: `isbn2wiki.toolforge.org` →
`https://magnustools.toolforge.org/isbn2wiki/` (2026-11-12).

## Finish, don't rewrite

- **PagePile**: done; the `tool` table row is now `rust/axum` (checked 2026-11-11). The PHP class stays for tools that include it.
- **WD-FIST**: the core is already Rust. Decide whether to retire/redirect the standalone PHP UI or expose the existing `petscan_rs` module; do not port the algorithm again. Legacy FIST is still best retired or pointed at FIST2; it still had ~207k uses in the 30 days to 2026-11-11, so redirect deliberately rather than removing the address.
- **WikiShootMe**: the map and search work is in the browser; PHP is OAuth, upload and API pass-through. It is still on php8.2 and had ~12k uses/30d. Fix the PHP in place (php8.2→8.4, timeouts on `file_get_contents`, OAuth token handling) instead of porting.
- **Reasonator**: do not port. V2 (`public_html/v2/`) is client-side JS; the PHP layer only handles OAuth and OpenGraph/Twitter metadata. The service answers 200, and the 2026-11-11 triage left 4 open rows (WIKI 54, 66, 315; Codeberg #40). Fix V2/autodesc, not the PHP shell.

## Ranking (new PHP ports)

| # | Tool | Use (30 days to 2026-11-11) | Why Rust helps | Effort | Status |
|---|---|---|---|---|---|
| 1 | most-wanted (`most-wanted/public_html/index.php`) | 19.3k | Live `max()` empty-array and 128 MB memory fatals; hints mode does per-article serial DB queries and unbounded template-user queries. Rust: read the `.links.red` file, batch the `IN` queries, cap hints. No exact Rust equivalent; `missingtopics` ranks red links for an article/category, not the global per-wiki list. | S–M | Rust port `src/tools/most_wanted.rs` (2026-11-11), with 11 daily `most-wanted-<wiki>` jobs in `jobs.yaml`. |
| 2 | isbn2wiki (`isbn2wiki/public_html/index.php`) | 176k | High traffic but currently healthy: already parallel with `curl_multi`; the 38k "assign property on string" fatals stopped 2026-10-28. Cheap consolidation: no PHP, native timeouts/retries, no internal HTTP logger call. Reuse the `wd-infernal` ISBN parsers; add DNB/LoC and wiki markup. | S–M | Rust port `src/tools/isbn2wiki.rs` (2026-11-12), using the published `bookmeta` crate (`~/rust/bookmeta`, https://github.com/magnusmanske/bookmeta). Sources: Google Books, DNB, K10plus, Library of Congress, Open Library, BnF, Crossref and Wikidata; plain line, `{{cite book}}`, `{{Literatur}}` and `{{Ouvrage}}`. |
| 3 | GLAMorous (`glamtools/public_html/glamorous.php`) | 7.5k | Loads every file name of a category tree (depth up to 20) to count them. Rust: stream and aggregate; reuse `baglama2_rs`'s `global_image_links` and category-tree code. Mostly a memory gain. | M | Still PHP 8.4. |
| 4 | tooltranslate API (`tooltranslate/public_html/api.php`) | 434 | Security, not performance: `update_json` writes under a path built from the request tool name; `add_tool` lets any logged-in user create a name with `../`; SQL strings are returned in responses; write modes lack an explicit CSRF token. Port only the write API (or fix in PHP). | S–M | Still PHP 8.4; 10 open issues (9 Codeberg + 1 WIKI). |

Ranked by expected gain, not raw traffic: isbn2wiki has more requests but is healthy and already parallel; most-wanted was the live bug fix and is now ported.

## Not worth porting

- **Legacy FIST `fist.php`**: the least stable code (~13.4k "max_user_connections" fatals/30d from opening a replica connection per language link, ~4.4k from unmapped wiki names), but many of its scrapers target dead sites. It still had ~207k uses in the 30 days to 2026-11-11, so point its JSON at FIST2 rather than removing it. Do not port 3.7k lines.
- **WikiShootMe**: the map work runs in the browser; PHP is OAuth and upload pass-through. A PHP cleanup (old `.user.ini`, php8.2) beats a port.
- **Wikidata Game**: the shared game API (`desc`/`tiles`/`log_action`) is already in `wikidata_todo_rs`, but the PHP `get_candidate` logic and batch jobs remain. With broken batch jobs and stale tickets, retire or fix the PHP tool rather than porting it whole; secure `update_game`/`update_all_games` meanwhile.
- **wikidata-terminator**: 0 logged uses; the current code validates language/type and reduces IDs to integers, so the old SQL-injection item is stale. Retire or fix in place, don't port.
- **commonshelper**: 32 uses/30d, 24 open issues (17 Codeberg + 7 WIKI; 2026-11-11), ~45 KB of PHP. Triage, retire, or redirect.
- **sourcemd** (0 logged uses, ongoing SPARQL failures; 32 open rows), **geograph2commons** (0 logged uses; 18 open rows): don't port. Reasonator is in "Finish, don't rewrite": V2 is JS, not a PHP port.
- **tooltranslate consumers**: consumers load static JSON from tools-static; only the editor is PHP. The fragile part for other tools is `tt.php` (`file_get_contents` without timeout). The editor API is a small security fix (see above); the rest is not worth porting.
- **GLAMorgan/TreeViews pageview proxy, BaGLAMa2 API**: thin or I/O-bound.
- **url2commons, flickr2commons**: only once an OAuth upload crate is shared (e.g. pulled out of `quickstatements_rs` into `wikimisc`). Both are on php8.4 now; the blocker is the shared upload code.
- **Other wikidata-todo endpoints** (`dupe_finder`, `beacon`, `true_duplicates`, `user_edits`, `add_name_labels`, `sexer`, `firstnamebasis`, `new_items`, …): several use removed tables or scan everything; retire rather than port. Several of these are already Rust in `wikidata_todo_rs`.
- **WikiFlix**: not analysed (no local checkout); redirects to its own tool now.
- **Zero/low-use PHP tools** (cradle, drawshield, filedupes, file-siblings, new-page-linker, pauken, picturesque, projector, quest, referee, tabernacle, toolflow, topicmatcher, wlmuk, catnap, multidesc, book2scroll, …): don't port unless they have a maintainer and real users.

## Fix now, Rust or not

Security:

- [ ] wikidata-terminator: the old SQL-injection note looks stale. The current `api.php`/`SparqlQueryBuilder` validates language/type and reduces IDs to integers (checked 2026-11-08). Confirm the deployed file matches, then close.
- [ ] Wikidata Game: `distributed/api.php` `update_game`/`update_all_games` are unauthenticated and call every registered game's `desc` endpoint. Require auth or run only from cron. The shared game API is Rust, but the classic game's candidate logic is still PHP.
- [ ] tooltranslate: `store_translation`/`add_tool` do require OAuth, but `update_json` is unauthenticated and builds the output path from the tool name. A logged-in user can add a name with `../`; writes are limited to `toolinfo.json`/`<language>.json` but can overwrite static JSON. Responses also include SQL strings. Fix in PHP now (typed names, no SQL in output, CSRF) whether or not it is ported.

Failing jobs:

- [ ] Listeria `update-wikis`: `./target/release/main: No such file or directory` since 2026-10-20.
- [ ] PagePile `cron-0` (`touch_data.sh`, old `tf-php74` image) failed; matches "Pile storage missing". The web service is Rust now and this repo's `jobs.yaml` has no pagepile job; confirm whether the old Toolforge job still exists and remove it if so.
- [ ] Wikidata Game `game-maintenance` and `update-distributed-game` failed.
- [ ] FIST: 13 jobs still on the PHP 7.4 image; the taxon media job has a 90 MB error log.
- [ ] Mix'n'match: no job progress since 2026-10-20 (last flagged by the monitor before its check moved to the dashboard).
- [ ] QuickStatements (Rust bot): many `Key "labels" not found` in `bot.out`.

Cheap PHP fixes for the worst errors:

- [x] most-wanted: ported; the `max()` guard is in `hint_categories`, hints/number are capped, and the template-usage query is batched.

- [ ] `ToolforgeCommon.php:705` (`getSPARQLitems` reads `$row->q` whatever the variable): the source of nearly all of the old wikidata-todo error log; the Rust `sparql_rc` should stop the growth, but other PHP tools still include this library.
- [ ] FIST `fist.php`: reuse one connection per replica cluster; map wiki names via the site matrix (`zh-min-nan`, `be-x-old`, `zh-yue`).
- [ ] GLAMorous: reject a non-list `projects` parameter (180 TypeErrors).

Other:

- [ ] Listeria: `query_list.php` has no Rust route; port or drop.
- [x] Tool table: `webserver_type` set from the live `service.manifest` for the 18 subdomains checked on 2026-10-23.
- [x] Tool table: usualsuspects and not-in-the-other-language rows updated on 2026-11-08 to `magnustools` / `rust/axum` with the new URLs.
- [x] Tool table: sighting rows updated on 2026-11-10 to `magnustools` / `rust/axum` with the new URLs; outofsight marked as deactivated.
- [x] Tool table: dnbtools rows updated on 2026-11-10 to `magnustools` / `rust/axum` with the new URLs.
- [x] Tool table: pagepile is now `rust/axum` (checked 2026-11-11).
- [ ] Tool table: catfood's main row is `rust/axum`, but the old `commons_image_feed` row still says `php8.4`; remove it or mark it retired.
- [ ] Tool table: erwin85 and joanjoc tombstone rows still say `php8.4`; their notes say the tools moved/redirect. Set them from `service.manifest` or mark them as redirects, not PHP services.
- [x] Reasonator: not a port target; V2 is JS and the PHP shell is OAuth/OpenGraph. The 503 is fixed and the backlog is down to 4 (2026-11-11).
- [ ] Remove the leftover `/tmp/wdt_tail.*` on `login.toolforge.org` (a copy of the wikidata-todo error log from this analysis; the host was unreachable).
