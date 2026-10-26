# PHP to Rust: which tools to convert

Triage of the tools on the [main page](https://magnustools.toolforge.org/) that are not Rust yet, by expected gain in efficiency, stability, speed and memory. As of 2026-10-23.

Based on: usage over 30 days (logger uses, dashboard requests), the code (`~/php/<tool>`), live error logs and job status on Toolforge. Toolforge SSH went down mid-analysis, so some log breakdowns are incomplete (marked "not verified"). Many PHP tools are down (503) after a PHP security incident; that is known and not counted against them.

## Already Rust
PetScan, Mix'n'match, QuickStatements (web and bot, since ~2026-10-21), Listeria (web, since 2026-10-23), geohack (9.2M requests/30d), autodesc, wdrc, Harvest Templates, wd-infernal, ac2wd, RustCCI, magnustools, catfood (since 2026-10-24), missingtopics (part of magnustools since 2026-10-24). add-information (part of magnustools since 2026-10-25). makeref (part of magnustools since 2026-10-26). BaGLAMa2 is ~95% Rust (`baglama2_rs`; only a thin read API and the add-category action are PHP).

The tool table's `webserver_type` was out of date for several of these; fixed 2026-10-23 (see below).

## Ranking

| # | Tool | Use (30 days) | Why Rust helps | Effort | Status |
|---|---|---|---|---|---|
| 1 | WD-FIST (`fist/public_html/wdfist/api.php`) | 307k uses | Hundreds of per-wiki `globalimagelinks` queries one after another into big nested arrays (1 GB, 5 min limits). Rust: concurrent queries on a pool, compact structs; expect several times faster, 5–10× less memory. Risk: Widar/OAuth for edits. | M | |
| 2 | PagePile | not logged | Infrastructure for PetScan (`render/pagepile.rs`, `datasource/pagepile.rs`) and `tools_interface`. 1,533 out-of-memory fatals in 2026; whole uploads and piles in memory; `run_filters` holds workers up to 20 min. Rust: steady memory, pooled connections, reliable timeouts. Keep all output formats; rusqlite reads the existing SQLite files; PHPUnit tests help with parity. | M | |
| 3 | Wikidata Game | not logged | Each `get_candidate`: 2+ DB connections, up to 1,000-row random scan, candidates checked one by one (`table=all`: 7 tables). Rust: prefetched candidate queue per table, pooled connections. Batch jobs are broken (`jsub`, removed `wb_terms`); port them too (L). The Distributed Game API is thin; little gain there. | M–L | |
| 4 | wikidata-todo `sparql_rc.php` | heavy | Most of the tool's fatals (SPARQL timeouts); 2.5 GB limit; unbounded `IN(...)` on `revision_compat`. Rust: stream SPARQL results, chunk lists, bounded concurrent diffs. Reuse `tools_interface/src/sparql_rc.rs`, maybe `wdrc_rs`. | M | |
| 5 | GLAMorous (`glamtools/public_html/glamorous.php`) | 8k uses | Loads every file name of a tree (depth up to 20) into memory just to count them. Rust: stream and aggregate. Mostly a memory gain; lower the max depth either way. | M | |

## Not worth porting
- **Legacy FIST `fist.php`**: the least stable code (~13.4k "max_user_connections" fatals/30d from opening a replica connection per language link, ~4.4k from unmapped wiki names), but many of its scrapers target dead sites. Retire it, or point its JSON at FIST2, instead of porting 3.7k lines.
- **WikiShootMe**: the map work runs in the browser; PHP is OAuth and upload pass-through. A PHP cleanup (old `.user.ini`, php8.2) beats a port.
- **tooltranslate**: consumers load static JSON from tools-static; PHP only serves the editor. The fragile part is `tt.php` in other tools (`file_get_contents` without timeout).
- **reasonator, wikidata-terminator, FIST2, GLAMorgan/TreeViews pageview proxy, BaGLAMa2 API**: thin or I/O-bound.
- **url2commons, flickr2commons**: only once an OAuth upload crate is shared (e.g. pulled out of `quickstatements_rs` into `wikimisc`). flickr2commons would also get off php7.3.
- **Other wikidata-todo endpoints** (`dupe_finder`, `beacon`, `true_duplicates`, `user_edits`, `add_name_labels`, `sexer`, `firstnamebasis`, `new_items`, …): several use removed tables or scan everything; retire rather than port.
- **WikiFlix**: not analysed (no local checkout).

## Fix now, Rust or not

Security:

- [x] wikidata-terminator: `Unknown column 'L623145' in 'where clause'` in the log suggests unquoted input in SQL.
- [ ] Wikidata Game: `update_all_games` is unauthenticated and calls every registered game's `desc` endpoint.
- [ ] tooltranslate: `store_translation` / `add_tool` in `api.php` may not require a login (not verified).

Failing jobs:
- [ ] Listeria `update-wikis`: `./target/release/main: No such file or directory` since 2026-10-20.
- [ ] PagePile `cron-0` (`touch_data.sh`, old `tf-php74` image) failed; matches "Pile storage missing".
- [ ] Wikidata Game `game-maintenance` and `update-distributed-game` failed.
- [ ] FIST: 13 jobs still on the PHP 7.4 image; the taxon media job has a 90 MB error log.
- [ ] Mix'n'match: no job progress since 2026-10-20 (last flagged by the monitor before its check moved to the dashboard).
- [ ] QuickStatements (Rust bot): many `Key "labels" not found` in `bot.out`.

Cheap PHP fixes for the worst errors:
- [ ] PagePile: HTML view of Wikidata piles loads full entities (`magnustools/php/wikidata.php:102`); fetch labels only and cap `max` (`api.php:19`).
- [ ] `ToolforgeCommon.php:705` (`getSPARQLitems` reads `$row->q` whatever the variable): nearly all of wikidata-todo's 1.6 GB error log.

- [ ] FIST `fist.php`: reuse one connection per replica cluster; map wiki names via the site matrix (`zh-min-nan`, `be-x-old`, `zh-yue`).
- [ ] GLAMorous: reject a non-list `projects` parameter (180 TypeErrors).

Other:
- [ ] Listeria: `query_list.php` has no Rust route; port or drop.
- [x] Tool table: `webserver_type` set from the live `service.manifest` for the 18 subdomains checked here (2026-10-23; restore SQL in the magnustools tool home). Other subdomains were not checked.
- [ ] Remove the leftover `/tmp/wdt_tail.*` on `login.toolforge.org` (a copy of the wikidata-todo error log from this analysis; the host was unreachable).
