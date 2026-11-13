//! The daily rebuild of the Terminator's `top_candidates` table. The old
//! PHP job scanned the Wikidata `page` table with three correlated
//! subqueries per row; this one keeps that index-friendly scan and batches
//! the term-store and SPARQL lookups.

use super::builder;
use super::query::sparql_client;
use crate::state::AppState;
use crate::wiki::Wiki;
use anyhow::{Context, Result};
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Params as DbParams, Value as DbValue};

const SCORE_FLOOR: i64 = 15;
/// Size of the global top-score pool kept in memory.
const TOP_K: usize = 50_000;
/// Rows kept per (type, language).
const PER_LANG: usize = 3_000;
const SCAN_BATCH: u64 = 5_000;
const ENRICH_BATCH: usize = 500;
const INSERT_BATCH: usize = 1_000;

/// Languages people use the tool for. The table scales linearly.
const LANGUAGES: &[&str] = &[
    "ta", "kn", "cy", "am", "eo", "ht", "or", "nap", "ast", "gu", "te", "ar", "ca", "ml", "nl", "ru", "en", "de", "fr",
    "es", "it", "hr", "uz", "he", "fi", "sv", "fa", "ro", "da", "cs", "pl", "zh", "hi", "nb",
];

/// P31 values considered "main" and not worth translating (scholarly
/// articles, genes, Wikimedia-internal pages, …). Seeded into `main_p31`.
const MAIN_P31: &[u32] = &[
    5, 4167410, 4167836, 4663903, 6964088, 11266439, 11753321, 13406463, 13442814, 14204246, 15184295, 15647814,
    17362920, 19842659, 19887878, 20010800, 20769160, 23838648, 23894233, 24046192, 30432511, 35250433, 36330215,
];

const TYPES: &[&str] = &["labels", "descriptions", "sites"];

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS `main_p31` (
        `q` INT UNSIGNED NOT NULL, PRIMARY KEY (`q`)
     ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
    "CREATE TABLE IF NOT EXISTS `meta` (
        `name` VARCHAR(64) NOT NULL, `value` VARCHAR(255) NOT NULL DEFAULT '', PRIMARY KEY (`name`)
     ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
    "CREATE TABLE IF NOT EXISTS `top_candidates` (
        `type` ENUM('labels','descriptions','sites') NOT NULL, `language` VARCHAR(35) NOT NULL,
        `q` INT UNSIGNED NOT NULL, `score` INT NOT NULL, `claims` INT NOT NULL, `identifiers` INT NOT NULL,
        `sitelinks` INT NOT NULL, `p31` INT UNSIGNED NOT NULL DEFAULT 0, `rnd` FLOAT NOT NULL,
        PRIMARY KEY (`type`,`language`,`q`), KEY `by_score` (`type`,`language`,`score`),
        KEY `by_claims` (`type`,`language`,`claims`), KEY `by_sitelinks` (`type`,`language`,`sitelinks`),
        KEY `by_rnd` (`type`,`language`,`rnd`)
     ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PoolItem {
    q: u32,
    claims: i64,
    identifiers: i64,
    sitelinks: i64,
    score: i64,
}

impl Ord for PoolItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score.cmp(&other.score).then_with(|| self.q.cmp(&other.q))
    }
}

impl PartialOrd for PoolItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct Item {
    item: PoolItem,
    p31: u32,
    labels: HashSet<String>,
    descriptions: HashSet<String>,
    sites: HashSet<String>,
}

impl Item {
    fn new(item: PoolItem) -> Self {
        Self { item, p31: 0, labels: HashSet::new(), descriptions: HashSet::new(), sites: HashSet::new() }
    }

    fn has(&self, search_type: &str, language: &str) -> bool {
        match search_type {
            "labels" => self.labels.contains(language),
            "descriptions" => self.descriptions.contains(language),
            _ => self.sites.contains(language),
        }
    }
}

/// Rebuilds the candidate index and swaps it in.
pub async fn refresh(state: &AppState) -> Result<()> {
    tracing::info!("scanning the Wikidata page table for the top {TOP_K} items by score");
    let pool = scan(state).await?;
    if pool.is_empty() {
        tracing::warn!("no Wikidata items passed the score floor; leaving the index alone");
        return Ok(());
    }
    tracing::info!("pool size: {}", pool.len());
    let items = enrich(state, &pool).await?;
    write(state, items).await?;
    Ok(())
}

async fn scan(state: &AppState) -> Result<Vec<PoolItem>> {
    let mut conn = state
        .replicas
        .conn(&Wiki::wikidata(), &["page", "page_props"])
        .await
        .context("connecting to the Wikidata replica")?;
    let mut heap: BinaryHeap<Reverse<PoolItem>> = BinaryHeap::new();
    let mut start = 0u64;
    let mut scanned = 0usize;
    loop {
        let rows: Vec<(u64, String, i64, i64, i64)> = conn
            .exec(
                "SELECT p.page_id, p.page_title,
                    COALESCE((SELECT CAST(pp_value AS SIGNED) FROM page_props WHERE pp_page=p.page_id AND pp_propname='wb-claims'),0) AS claims,
                    COALESCE((SELECT CAST(pp_value AS SIGNED) FROM page_props WHERE pp_page=p.page_id AND pp_propname='wb-identifiers'),0) AS identifiers,
                    COALESCE((SELECT CAST(pp_value AS SIGNED) FROM page_props WHERE pp_page=p.page_id AND pp_propname='wb-sitelinks'),0) AS sitelinks
                 FROM page p
                 WHERE p.page_namespace=0 AND p.page_is_redirect=0 AND p.page_id>?
                 HAVING claims+sitelinks-identifiers>=?
                 ORDER BY p.page_id LIMIT ?",
                (start, SCORE_FLOOR, SCAN_BATCH),
            )
            .await
            .with_context(|| format!("scanning Wikidata page_id > {start}"))?;
        let count = rows.len();
        for (page_id, title, claims, identifiers, sitelinks) in rows {
            start = page_id;
            let Some(q) = builder::item_number(&title) else { continue };
            let item = PoolItem { q, claims, identifiers, sitelinks, score: claims + sitelinks - identifiers };
            if heap.len() < TOP_K {
                heap.push(Reverse(item));
            } else if heap.peek().is_some_and(|smallest| item.score > smallest.0.score) {
                heap.pop();
                heap.push(Reverse(item));
            }
        }
        scanned += count;
        if scanned % 100_000 < SCAN_BATCH as usize {
            tracing::info!("scanned {scanned} qualifying items, pool={}", heap.len());
        }
        if count < SCAN_BATCH as usize {
            break;
        }
    }
    Ok(heap.into_iter().map(|item| item.0).collect())
}

async fn enrich(state: &AppState, pool: &[PoolItem]) -> Result<HashMap<u32, Item>> {
    tracing::info!("enriching the pool with labels, descriptions, sitelinks and P31");
    let mut items: HashMap<u32, Item> = pool.iter().map(|item| (item.q, Item::new(*item))).collect();
    let mut terms = state
        .replicas
        .conn(&Wiki::wikidata(), &["wbt_item_terms", "wbt_term_in_lang", "wbt_text_in_lang"])
        .await
        .context("connecting to the Wikidata term store")?;
    terms.query_drop("SET SESSION group_concat_max_len = 1000000").await?;
    let mut sites = state
        .replicas
        .conn(&Wiki::wikidata(), &["wb_items_per_site"])
        .await
        .context("connecting to the Wikidata sitelinks table")?;
    let site_to_language: HashMap<String, String> =
        LANGUAGES.iter().map(|language| (builder::wikipedia_site_id(language), (*language).to_string())).collect();
    let site_ids: Vec<String> = site_to_language.keys().cloned().collect();
    let site_placeholders = crate::replicas::placeholders(site_ids.len());
    let language_placeholders = crate::replicas::placeholders(LANGUAGES.len());
    let sparql = sparql_client().context("creating the WDQS client")?;

    for chunk in pool.chunks(ENRICH_BATCH) {
        let ids: Vec<u32> = chunk.iter().map(|item| item.q).collect();
        let id_values: Vec<DbValue> = ids.iter().map(|q| DbValue::from(*q)).collect();
        let id_placeholders = crate::replicas::placeholders(ids.len());

        for type_id in [1u8, 2u8] {
            let sql = format!(
                "SELECT wbit_item_id AS q, GROUP_CONCAT(wbxl_language) AS langs
                 FROM wbt_item_terms
                 INNER JOIN wbt_term_in_lang ON wbit_term_in_lang_id=wbtl_id AND wbtl_type_id=?
                 INNER JOIN wbt_text_in_lang ON wbtl_text_in_lang_id=wbxl_id AND wbxl_language IN ({language_placeholders})
                 WHERE wbit_item_id IN ({id_placeholders})
                 GROUP BY wbit_item_id"
            );
            let mut values = vec![DbValue::from(type_id)];
            values.extend(LANGUAGES.iter().map(|language| DbValue::from(*language)));
            values.extend(id_values.iter().cloned());
            let rows: Vec<(u32, String)> = terms.exec(sql, DbParams::Positional(values)).await?;
            for (q, languages) in rows {
                let Some(item) = items.get_mut(&q) else { continue };
                let target = if type_id == 1 { &mut item.labels } else { &mut item.descriptions };
                target.extend(languages.split(',').map(str::to_string));
            }
        }

        let sql = format!(
            "SELECT ips_item_id AS q, GROUP_CONCAT(ips_site_id) AS sites
             FROM wb_items_per_site WHERE ips_item_id IN ({id_placeholders}) AND ips_site_id IN ({site_placeholders})
             GROUP BY ips_item_id"
        );
        let mut values = id_values.clone();
        values.extend(site_ids.iter().map(|site| DbValue::from(site.as_str())));
        let rows: Vec<(u32, String)> = sites.exec(sql, DbParams::Positional(values)).await?;
        for (q, site_list) in rows {
            let Some(item) = items.get_mut(&q) else { continue };
            for site in site_list.split(',') {
                if let Some(language) = site_to_language.get(site) {
                    item.sites.insert(language.clone());
                }
            }
        }

        let values = ids.iter().map(|q| format!("wd:Q{q}")).collect::<Vec<_>>().join(" ");
        let query = format!(
            "SELECT ?q (SAMPLE(?p) AS ?p31) WHERE {{ VALUES ?q {{ {values} }} OPTIONAL {{ ?q wdt:P31 ?p }} }} GROUP BY ?q"
        );
        let json = sparql.query_json(&query).await?;
        for binding in json["results"]["bindings"].as_array().into_iter().flatten() {
            let Some(q) = binding["q"]["value"].as_str().and_then(builder::item_number) else { continue };
            let p31 = binding["p31"]["value"].as_str().and_then(builder::item_number).unwrap_or(0);
            if let Some(item) = items.get_mut(&q) {
                item.p31 = p31;
            }
        }
    }
    Ok(items)
}

async fn write(state: &AppState, items: HashMap<u32, Item>) -> Result<()> {
    let mut items: Vec<Item> = items.into_values().collect();
    items.sort_by(|a, b| b.item.score.cmp(&a.item.score).then_with(|| a.item.q.cmp(&b.item.q)));
    let mut conn = state.terminator_db.conn().await.context("connecting to the terminator database")?;
    for statement in SCHEMA {
        conn.query_drop(*statement).await?;
    }
    let seed = MAIN_P31.iter().map(|q| format!("({q})")).collect::<Vec<_>>().join(",");
    conn.query_drop(format!("INSERT IGNORE INTO main_p31 (q) VALUES {seed}")).await?;

    conn.query_drop("DROP TABLE IF EXISTS top_candidates_build").await?;
    conn.query_drop("CREATE TABLE top_candidates_build LIKE top_candidates").await?;

    let mut rng = rand::rng();
    for search_type in TYPES {
        for language in LANGUAGES {
            let mut batch: Vec<Item> = Vec::with_capacity(INSERT_BATCH);
            let mut rows = 0usize;
            for item in &items {
                if rows >= PER_LANG {
                    break;
                }
                if item.has(search_type, language) {
                    continue;
                }
                batch.push(Item {
                    item: item.item,
                    p31: item.p31,
                    labels: HashSet::new(),
                    descriptions: HashSet::new(),
                    sites: HashSet::new(),
                });
                rows += 1;
                if batch.len() >= INSERT_BATCH {
                    insert_batch(&mut conn, search_type, language, &batch, &mut rng).await?;
                    batch.clear();
                }
            }
            if !batch.is_empty() {
                insert_batch(&mut conn, search_type, language, &batch, &mut rng).await?;
            }
            tracing::info!("{search_type}/{language}: {rows} rows");
        }
    }

    conn.query_drop("DROP TABLE IF EXISTS top_candidates_old").await?;
    conn.query_drop("RENAME TABLE top_candidates TO top_candidates_old, top_candidates_build TO top_candidates")
        .await?;
    conn.query_drop("DROP TABLE IF EXISTS top_candidates_old").await?;
    conn.query_drop(
        "REPLACE INTO meta (name,value) VALUES ('last_refresh', DATE_FORMAT(UTC_TIMESTAMP(), '%Y%m%d%H%i%s'))",
    )
    .await?;
    tracing::info!("terminator candidate index refreshed");
    Ok(())
}

async fn insert_batch(
    conn: &mut Conn,
    search_type: &str,
    language: &str,
    batch: &[Item],
    rng: &mut impl rand::Rng,
) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let row = "(?,?,?,?,?,?,?,?,?)";
    let sql = format!(
        "INSERT INTO top_candidates_build (type,language,q,score,claims,identifiers,sitelinks,p31,rnd) VALUES {}",
        vec![row; batch.len()].join(",")
    );
    let mut values = Vec::with_capacity(batch.len() * 9);
    for item in batch {
        values.push(DbValue::from(search_type));
        values.push(DbValue::from(language));
        values.push(DbValue::from(item.item.q));
        values.push(DbValue::from(item.item.score as i32));
        values.push(DbValue::from(item.item.claims as i32));
        values.push(DbValue::from(item.item.identifiers as i32));
        values.push(DbValue::from(item.item.sitelinks as i32));
        values.push(DbValue::from(item.p31));
        values.push(DbValue::from(rng.random::<f32>()));
    }
    conn.exec_drop(sql, DbParams::Positional(values)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_keeps_the_largest_scores() {
        let mut heap: BinaryHeap<Reverse<PoolItem>> = BinaryHeap::new();
        for (q, score) in [(1, 20), (2, 30), (3, 10)] {
            let item = PoolItem { q, claims: 0, identifiers: 0, sitelinks: 0, score };
            if heap.len() < 2 {
                heap.push(Reverse(item));
            } else if heap.peek().is_some_and(|smallest| item.score > smallest.0.score) {
                heap.pop();
                heap.push(Reverse(item));
            }
        }
        let mut qs: Vec<u32> = heap.into_iter().map(|item| item.0.q).collect();
        qs.sort_unstable();
        assert_eq!(qs, vec![1, 2]);
    }

    #[test]
    fn item_membership_matches_the_type() {
        let item = Item {
            item: PoolItem { q: 1, claims: 0, identifiers: 0, sitelinks: 0, score: 0 },
            p31: 0,
            labels: HashSet::from(["en".to_string()]),
            descriptions: HashSet::new(),
            sites: HashSet::new(),
        };
        assert!(item.has("labels", "en"));
        assert!(!item.has("labels", "de"));
        assert!(!item.has("descriptions", "en"));
    }
}
