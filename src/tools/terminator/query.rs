//! Candidate search for the Wikidata Terminator: live against WDQS when the
//! request has a SPARQL seed, otherwise from the daily `top_candidates` table.

use super::builder;
use super::{Mode, SearchType, SortField};
use crate::state::AppState;
use crate::web::AppError;
use rand::Rng;
use serde::Serialize;
use std::collections::HashSet;
use std::time::Duration;
use wikimisc::cache::TimedCache;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Params as DbParams, Value as DbValue};
use wikimisc::sparql_client::SparqlClient;
use wikimisc::sparql_endpoint::SparqlEndpoint;

const MIN_SCORE: i64 = 15;
/// Candidates pulled from `top_candidates` before a present-language check.
const PRESENT_WORKING_SET: usize = 5_000;
const PRESENCE_CHUNK: usize = 1_000;
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

static MAIN_P31: TimedCache<Vec<u32>> = TimedCache::new(Duration::from_secs(3_600));
static LANGUAGES: TimedCache<Vec<(String, String)>> = TimedCache::new(Duration::from_secs(86_400));

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub q: u32,
    pub claims: i64,
    pub identifiers: i64,
    pub sitelinks: i64,
    pub score: i64,
    pub p31: u32,
}

#[derive(Debug)]
pub struct Search {
    pub results: Vec<Candidate>,
    pub offset: usize,
    pub mode: Mode,
    pub tier: Option<&'static str>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SearchParams {
    pub search_type: SearchType,
    pub missing_languages: Vec<String>,
    pub present_languages: Vec<String>,
    pub sparql: String,
    pub sort: SortField,
    pub show_untranslatables: bool,
    pub max_results: usize,
    pub offset: usize,
    pub mode: Mode,
}

pub async fn run(state: &AppState, params: &SearchParams) -> Result<Search, AppError> {
    let mut out = Search { results: vec![], offset: params.offset, mode: params.mode, tier: None, note: None };
    if params.missing_languages.is_empty() {
        out.note = Some("Select at least one (valid) language to find missing terms in.".to_string());
        return Ok(out);
    }
    let exclude_p31 = if params.show_untranslatables { None } else { Some(main_p31_ids(state).await?) };
    if !params.sparql.trim().is_empty() {
        out.tier = Some("live");
        out.results = run_live(params, exclude_p31.as_deref()).await?;
    } else {
        out.tier = Some("precomputed");
        out.results = run_precomputed(state, params, exclude_p31.as_deref(), &mut out.note).await?;
    }
    out.offset = params.offset + out.results.len();
    Ok(out)
}

async fn run_live(params: &SearchParams, exclude_p31: Option<&[u32]>) -> Result<Vec<Candidate>, AppError> {
    let game = params.mode == Mode::Game;
    let limit = if game { params.max_results.max(50) } else { params.max_results };
    let query = builder::build_live_query(
        params.search_type,
        &params.missing_languages,
        &params.present_languages,
        &params.sparql,
        exclude_p31,
        MIN_SCORE,
        params.sort,
        limit,
        if game { 0 } else { params.offset },
    )
    .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let json = sparql_client()?.query_json(&query).await.map_err(|e| AppError::Upstream(anyhow::Error::new(e)))?;
    let results = parse_bindings(&json);
    Ok(if game { pick_random(results, 1) } else { results })
}

async fn run_precomputed(
    state: &AppState,
    params: &SearchParams,
    exclude_p31: Option<&[u32]>,
    note: &mut Option<String>,
) -> Result<Vec<Candidate>, AppError> {
    let mut conn = state.terminator_db.conn().await?;
    if !top_candidates_ready(&mut conn).await? {
        *note = Some(
            "The candidate index has not been built yet. Either run a SPARQL-restricted search, or wait for the daily refresh."
                .to_string(),
        );
        return Ok(vec![]);
    }
    if !has_precomputed_language(&mut conn, params).await? {
        *note = Some(format!(
            "No precomputed candidates for {}. Add a SPARQL restriction to search these languages live.",
            params.missing_languages.join(", ")
        ));
        return Ok(vec![]);
    }
    let (where_sql, params_db) = precomputed_where(params, exclude_p31);
    let game = params.mode == Mode::Game;

    if params.present_languages.is_empty() {
        if game {
            return precomputed_random(&mut conn, &where_sql, &params_db).await;
        }
        let sql = format!(
            "SELECT q, MAX(score) AS score, MAX(claims) AS claims, MAX(identifiers) AS identifiers, \
             MAX(sitelinks) AS sitelinks, MAX(p31) AS p31 FROM top_candidates WHERE {where_sql} \
             GROUP BY q ORDER BY {} DESC, q LIMIT ? OFFSET ?",
            params.sort.as_str()
        );
        let mut values = params_db;
        values.push(DbValue::from(params.max_results as u64));
        values.push(DbValue::from(params.offset as u64));
        return rows_from_sql(&mut conn, sql, values).await;
    }

    let sql = format!(
        "SELECT q, MAX(score) AS score, MAX(claims) AS claims, MAX(identifiers) AS identifiers, \
         MAX(sitelinks) AS sitelinks, MAX(p31) AS p31 FROM top_candidates WHERE {where_sql} \
         GROUP BY q ORDER BY {} DESC, q LIMIT {PRESENT_WORKING_SET}",
        params.sort.as_str()
    );
    let candidates = rows_from_sql(&mut conn, sql, params_db).await?;
    if candidates.len() >= PRESENT_WORKING_SET {
        *note = Some(format!("Present-language results are limited to the top {PRESENT_WORKING_SET} candidates."));
    }
    let have = filter_by_presence(&candidates, params).await?;
    let candidates: Vec<Candidate> = candidates.into_iter().filter(|candidate| have.contains(&candidate.q)).collect();
    if game {
        return Ok(pick_random(candidates, 1));
    }
    Ok(candidates.into_iter().skip(params.offset).take(params.max_results).collect())
}

fn precomputed_where(params: &SearchParams, exclude_p31: Option<&[u32]>) -> (String, Vec<DbValue>) {
    let placeholders = crate::replicas::placeholders(params.missing_languages.len());
    let mut sql = format!("type=? AND language IN ({placeholders})");
    let mut values = vec![DbValue::from(params.search_type.as_str())];
    values.extend(params.missing_languages.iter().map(|language| DbValue::from(language.as_str())));
    if let Some(p31) = exclude_p31.filter(|p31| !p31.is_empty()) {
        let placeholders = crate::replicas::placeholders(p31.len());
        sql.push_str(&format!(" AND p31 NOT IN ({placeholders})"));
        values.extend(p31.iter().map(|q| DbValue::from(*q)));
    }
    (sql, values)
}

async fn top_candidates_ready(conn: &mut Conn) -> Result<bool, AppError> {
    let row: Option<(String,)> =
        conn.exec_first("SHOW TABLES LIKE 'top_candidates'", ()).await.map_err(anyhow::Error::from)?;
    Ok(row.is_some())
}

async fn has_precomputed_language(conn: &mut Conn, params: &SearchParams) -> Result<bool, AppError> {
    let (where_sql, values) = precomputed_where(params, None);
    let sql = format!("SELECT 1 FROM top_candidates WHERE {where_sql} LIMIT 1");
    let row: Option<(i64,)> = conn.exec_first(sql, DbParams::Positional(values)).await.map_err(anyhow::Error::from)?;
    Ok(row.is_some())
}

async fn precomputed_random(conn: &mut Conn, where_sql: &str, values: &[DbValue]) -> Result<Vec<Candidate>, AppError> {
    let sql = format!(
        "SELECT q, score, claims, identifiers, sitelinks, p31 FROM top_candidates \
         WHERE {where_sql} AND rnd>=? ORDER BY rnd LIMIT 1"
    );
    let mut random = values.to_vec();
    random.push(DbValue::from(rand::random::<f32>()));
    let rows = rows_from_sql(conn, sql, random).await?;
    if !rows.is_empty() {
        return Ok(rows);
    }
    let sql = format!(
        "SELECT q, score, claims, identifiers, sitelinks, p31 FROM top_candidates \
         WHERE {where_sql} ORDER BY rnd LIMIT 1"
    );
    rows_from_sql(conn, sql, values.to_vec()).await
}

async fn rows_from_sql(conn: &mut Conn, sql: String, values: Vec<DbValue>) -> Result<Vec<Candidate>, AppError> {
    let rows: Vec<(u32, i64, i64, i64, i64, u32)> =
        conn.exec(sql, DbParams::Positional(values)).await.map_err(anyhow::Error::from)?;
    Ok(rows
        .into_iter()
        .map(|(q, score, claims, identifiers, sitelinks, p31)| Candidate {
            q,
            claims,
            identifiers,
            sitelinks,
            score,
            p31,
        })
        .collect())
}

async fn filter_by_presence(candidates: &[Candidate], params: &SearchParams) -> Result<HashSet<u32>, AppError> {
    let mut have = HashSet::new();
    if candidates.is_empty() {
        return Ok(have);
    }
    let client = sparql_client()?;
    for chunk in candidates.chunks(PRESENCE_CHUNK) {
        let ids: Vec<u32> = chunk.iter().map(|candidate| candidate.q).collect();
        let query = builder::build_presence_query(&ids, params.search_type, &params.present_languages);
        let json = client.query_json(&query).await.map_err(|e| AppError::Upstream(anyhow::Error::new(e)))?;
        for binding in json["results"]["bindings"].as_array().into_iter().flatten() {
            if let Some(q) = binding["q"]["value"].as_str().and_then(builder::item_number) {
                have.insert(q);
            }
        }
    }
    Ok(have)
}

fn parse_bindings(json: &serde_json::Value) -> Vec<Candidate> {
    json["results"]["bindings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|binding| {
            let value = |name: &str| binding[name]["value"].as_str().unwrap_or_default();
            let q = builder::item_number(value("q"))?;
            Some(Candidate {
                q,
                claims: value("claims").parse().unwrap_or(0),
                identifiers: value("identifiers").parse().unwrap_or(0),
                sitelinks: value("sitelinks").parse().unwrap_or(0),
                score: value("score").parse().unwrap_or(0),
                p31: builder::item_number(value("p31s")).unwrap_or(0),
            })
        })
        .collect()
}

fn pick_random(rows: Vec<Candidate>, count: usize) -> Vec<Candidate> {
    if rows.len() <= count {
        return rows;
    }
    let mut chosen = HashSet::new();
    let mut rng = rand::rng();
    while chosen.len() < count {
        chosen.insert(rng.random_range(0..rows.len()));
    }
    rows.into_iter().enumerate().filter(|(index, _)| chosen.contains(index)).map(|(_, row)| row).collect()
}

pub(super) fn sparql_client() -> Result<SparqlClient, AppError> {
    SparqlClient::new(SparqlEndpoint::wikidata_main(), crate::state::USER_AGENT)
        .map(|client| client.with_retries(3).with_max_response_bytes(MAX_RESPONSE_BYTES))
        .map_err(|e| AppError::Upstream(anyhow::Error::new(e)))
}

async fn main_p31_ids(state: &AppState) -> Result<Vec<u32>, AppError> {
    MAIN_P31
        .get_or_fetch(async || {
            let mut conn = state.terminator_db.conn().await?;
            let table: Option<(String,)> = conn.exec_first("SHOW TABLES LIKE 'main_p31'", ()).await?;
            if table.is_none() {
                tracing::warn!("the terminator database has no main_p31 table; no type is excluded");
                return Ok(vec![]);
            }
            let rows: Vec<(u32,)> = conn.query("SELECT q FROM main_p31").await?;
            Ok(rows.into_iter().map(|(q,)| q).collect())
        })
        .await
        .map(|ids| (*ids).clone())
        .map_err(AppError::Upstream)
}

pub async fn languages(state: &AppState) -> Result<Vec<(String, String)>, AppError> {
    LANGUAGES
        .get_or_fetch(async || {
            let json = crate::mediawiki::api(
                &state.http,
                &crate::wiki::Wiki::wikidata(),
                &[("action", "query"), ("meta", "siteinfo"), ("siprop", "languages")],
            )
            .await?;
            let mut languages: Vec<(String, String)> = json["query"]["languages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|language| {
                    let code = language["code"].as_str()?;
                    let name = language["name"].as_str().or_else(|| language["*"].as_str()).unwrap_or(code);
                    Some((code.to_string(), name.to_string()))
                })
                .collect();
            languages.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
            Ok(languages)
        })
        .await
        .map(|languages| (*languages).clone())
        .map_err(AppError::Upstream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_become_candidates() {
        let json = serde_json::json!({
            "results": {"bindings": [
                {"q": {"value": "http://www.wikidata.org/entity/Q42"},
                 "claims": {"value": "10"}, "identifiers": {"value": "2"},
                 "sitelinks": {"value": "3"}, "score": {"value": "11"},
                 "p31s": {"value": "http://www.wikidata.org/entity/Q5"}},
                {"q": {"value": "http://www.wikidata.org/entity/L1"}}
            ]}
        });
        let candidates = parse_bindings(&json);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], Candidate { q: 42, claims: 10, identifiers: 2, sitelinks: 3, score: 11, p31: 5 });
    }

    #[test]
    fn where_clause_uses_parameters() {
        let params = SearchParams {
            search_type: SearchType::Labels,
            missing_languages: vec!["cy".into(), "en".into()],
            present_languages: vec![],
            sparql: String::new(),
            sort: SortField::Score,
            show_untranslatables: false,
            max_results: 50,
            offset: 0,
            mode: Mode::Top,
        };
        let (sql, values) = precomputed_where(&params, Some(&[5, 6]));
        assert_eq!(sql, "type=? AND language IN (?,?) AND p31 NOT IN (?,?)");
        assert_eq!(values.len(), 5);
    }
}
