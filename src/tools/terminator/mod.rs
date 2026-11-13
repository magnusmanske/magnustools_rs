//! Wikidata Terminator: items missing a label, description or sitelink in a
//! language, optionally restricted by a SPARQL query.
//!
//! The server-side page replaces the old Vue app; `api.php` keeps the old
//! read API. The daily `terminator-refresh` job rebuilds the precomputed
//! candidate index.

mod builder;
mod query;
mod update;

use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::encode_component;
use askama::Template;
use axum::Json;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

pub(crate) use update::refresh;

const DEFAULT_MAX_RESULTS: usize = 50;
const MAX_RESULTS: usize = 500;
const MAX_LANGUAGES: usize = 20;
/// Languages for which labels and descriptions are loaded for display.
const DISPLAY_LANGUAGES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchType {
    Labels,
    Descriptions,
    Sites,
}

impl SearchType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Labels => "labels",
            Self::Descriptions => "descriptions",
            Self::Sites => "sites",
        }
    }

    fn parse(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "descriptions" => Self::Descriptions,
            "sites" => Self::Sites,
            _ => Self::Labels,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Score,
    Claims,
    Identifiers,
    Sitelinks,
}

impl SortField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Score => "score",
            Self::Claims => "claims",
            Self::Identifiers => "identifiers",
            Self::Sitelinks => "sitelinks",
        }
    }

    fn parse(value: &str) -> Self {
        match value.trim().to_lowercase().as_str() {
            "claims" => Self::Claims,
            "identifiers" | "ext_ids" => Self::Identifiers,
            "sitelinks" => Self::Sitelinks,
            _ => Self::Score,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Top,
    Game,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Game => "game",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Request {
    action: Option<String>,
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    languages: String,
    #[serde(default)]
    present: String,
    #[serde(default)]
    sparql: String,
    #[serde(default)]
    sort_by: String,
    sui: Option<String>,
    max_results: Option<String>,
    offset: Option<String>,
    mode: Option<String>,
}

impl Request {
    fn flag(value: Option<&str>) -> bool {
        value.is_some_and(|v| !v.is_empty() && v != "0")
    }

    fn max_results(&self) -> usize {
        self.max_results
            .as_deref()
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .clamp(1, MAX_RESULTS)
    }

    fn offset(&self) -> usize {
        self.offset.as_deref().and_then(|n| n.trim().parse().ok()).unwrap_or(0)
    }

    fn search_type(&self) -> SearchType {
        SearchType::parse(&self.r#type)
    }

    fn sort(&self) -> SortField {
        SortField::parse(&self.sort_by)
    }

    fn mode(&self) -> Mode {
        if self.mode.as_deref().is_some_and(|m| m.trim().eq_ignore_ascii_case("game")) { Mode::Game } else { Mode::Top }
    }

    fn missing_languages(&self) -> Vec<String> {
        parse_languages(&self.languages)
    }

    fn present_languages(&self) -> Vec<String> {
        parse_languages(&self.present)
    }

    fn params(&self) -> query::SearchParams {
        query::SearchParams {
            search_type: self.search_type(),
            missing_languages: self.missing_languages(),
            present_languages: self.present_languages(),
            sparql: self.sparql.trim().to_string(),
            sort: self.sort(),
            show_untranslatables: Self::flag(self.sui.as_deref()),
            max_results: self.max_results(),
            offset: self.offset(),
            mode: self.mode(),
        }
    }
}

/// Language codes from a comma-separated list, lowercased, deduplicated and
/// limited in number.
fn parse_languages(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|language| language.trim().to_lowercase())
        .filter(|language| builder::valid_language(language))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_LANGUAGES)
        .collect()
}

struct DisplayRow {
    qid: String,
    url: String,
    label: String,
    description: String,
    p31: u32,
    p31_id: String,
    p31_url: String,
    p31_label: String,
    claims: i64,
    identifiers: i64,
    sitelinks: i64,
    score: i64,
}

struct DisplaySearch {
    rows: Vec<DisplayRow>,
    note: Option<String>,
    tier: Option<&'static str>,
    next_url: String,
    prev_url: Option<String>,
}

#[derive(Template)]
#[template(path = "tools/wikidata_terminator.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    search_type: &'static str,
    sort: &'static str,
    mode: &'static str,
    result_choices: Vec<(usize, bool)>,
    missing_languages: String,
    present_languages: String,
    search: Option<DisplaySearch>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let missing_languages = request.missing_languages();
    let mut page = Page {
        title: "Wikidata Terminator",
        request: &request,
        search_type: request.search_type().as_str(),
        sort: request.sort().as_str(),
        mode: request.mode().as_str(),
        result_choices: [50, 100, 250, 500].map(|n| (n, n == request.max_results())).to_vec(),
        missing_languages: request.languages.clone(),
        present_languages: request.present.clone(),
        search: None,
        error: None,
    };
    if missing_languages.is_empty() {
        return render(&page);
    }
    let params = request.params();
    match query::run(&state, &params).await {
        Ok(search) => {
            let rows = display_rows(&state, &search.results, &params).await;
            page.search = Some(DisplaySearch {
                rows,
                note: search.note,
                tier: search.tier,
                next_url: page_url(&request, search.offset),
                prev_url: (params.offset > 0)
                    .then(|| page_url(&request, params.offset.saturating_sub(params.max_results))),
            });
        }
        Err(e) => page.error = Some(public_error(&e)),
    }
    render(&page)
}

async fn display_rows(
    state: &AppState,
    candidates: &[query::Candidate],
    params: &query::SearchParams,
) -> Vec<DisplayRow> {
    let mut ids: Vec<u32> = candidates.iter().map(|candidate| candidate.q).collect();
    ids.extend(candidates.iter().filter(|candidate| candidate.p31 > 0).map(|candidate| candidate.p31));
    ids.sort_unstable();
    ids.dedup();
    let mut display_languages = vec!["en".to_string()];
    display_languages.extend(params.missing_languages.iter().take(MAX_LANGUAGES).cloned());
    display_languages.extend(params.present_languages.iter().take(MAX_LANGUAGES).cloned());
    display_languages.sort();
    display_languages.dedup();
    display_languages.truncate(DISPLAY_LANGUAGES);
    let entities = entity_info(&state.http, &ids, &display_languages).await;

    candidates
        .iter()
        .map(|candidate| {
            let (label, description) = entities.get(&candidate.q).cloned().unwrap_or_default();
            let (p31_label, _) = entities.get(&candidate.p31).cloned().unwrap_or_default();
            DisplayRow {
                qid: format!("Q{}", candidate.q),
                url: format!("https://www.wikidata.org/wiki/Q{}", candidate.q),
                label: label.unwrap_or_else(|| format!("Q{}", candidate.q)),
                description: description.unwrap_or_default(),
                p31: candidate.p31,
                p31_id: if candidate.p31 > 0 { format!("Q{}", candidate.p31) } else { String::new() },
                p31_url: format!("https://www.wikidata.org/wiki/Q{}", candidate.p31),
                p31_label: p31_label.unwrap_or_default(),
                claims: candidate.claims,
                identifiers: candidate.identifiers,
                sitelinks: candidate.sitelinks,
                score: candidate.score,
            }
        })
        .collect()
}

/// Labels and descriptions for `ids`, in the requested languages.
async fn entity_info(
    http: &reqwest::Client,
    ids: &[u32],
    languages: &[String],
) -> HashMap<u32, (Option<String>, Option<String>)> {
    let mut info = HashMap::new();
    if ids.is_empty() {
        return info;
    }
    let languages = languages.join("|");
    for chunk in ids.chunks(50) {
        let ids: Vec<String> = chunk.iter().map(|q| format!("Q{q}")).collect();
        let ids = ids.join("|");
        let json = crate::mediawiki::api(
            http,
            &crate::wiki::Wiki::wikidata(),
            &[
                ("action", "wbgetentities"),
                ("ids", ids.as_str()),
                ("props", "labels|descriptions"),
                ("languages", languages.as_str()),
                ("languagefallback", "1"),
            ],
        )
        .await;
        let Ok(json) = json else {
            tracing::warn!("cannot load entity labels for the Terminator results");
            continue;
        };
        for (id, entity) in json["entities"].as_object().into_iter().flatten() {
            let Some(q) = id.strip_prefix('Q').and_then(|q| q.parse().ok()) else { continue };
            let label = pick_term(&entity["labels"]);
            let description = pick_term(&entity["descriptions"]);
            info.insert(q, (label, description));
        }
    }
    info
}

fn pick_term(terms: &Value) -> Option<String> {
    let terms = terms.as_object()?;
    terms
        .get("en")
        .and_then(|term| term["value"].as_str())
        .or_else(|| terms.values().find_map(|term| term["value"].as_str()))
        .map(str::to_string)
}

fn public_error(error: &AppError) -> String {
    match error {
        AppError::BadRequest(message) => message.clone(),
        _ => "The search failed. Please try again later.".to_string(),
    }
}

fn page_url(request: &Request, offset: usize) -> String {
    let mut url = format!(
        "/wikidata-terminator/?type={}&languages={}&present={}&sort_by={}&sui={}&max_results={}&offset={}&mode={}",
        encode_component(request.search_type().as_str()),
        encode_component(&request.languages),
        encode_component(&request.present),
        encode_component(request.sort().as_str()),
        u8::from(Request::flag(request.sui.as_deref())),
        request.max_results(),
        offset,
        encode_component(request.mode().as_str())
    );
    if !request.sparql.trim().is_empty() {
        url.push_str("&sparql=");
        url.push_str(&encode_component(&request.sparql));
    }
    url
}

/// `api.php`: `action=languages` and `action=run_query`, as the old Vue app
/// and any outside callers use them.
pub async fn api(State(state): State<Arc<AppState>>, Params(request): Params<Request>) -> Json<Value> {
    let out = match request.action.as_deref().unwrap_or_default() {
        "languages" => match query::languages(&state).await {
            Ok(languages) => {
                let languages: serde_json::Map<String, Value> =
                    languages.into_iter().map(|(code, name)| (code, Value::String(name))).collect();
                json!({"status": "OK", "languages": languages})
            }
            Err(e) => json!({"status": format!("ERROR: {}", public_error(&e))}),
        },
        "run_query" => {
            let mut params = request.params();
            if request.languages.trim().is_empty() {
                params.missing_languages = vec!["en".to_string()];
            }
            match query::run(&state, &params).await {
                Ok(search) => {
                    let mut value = json!({
                        "status": "OK",
                        "results": search.results,
                        "offset": search.offset,
                        "mode": search.mode.as_str(),
                    });
                    if let Some(tier) = search.tier {
                        value["tier"] = Value::String(tier.to_string());
                    }
                    if let Some(note) = search.note {
                        value["note"] = Value::String(note);
                    }
                    value
                }
                Err(e) => json!({"status": format!("ERROR: {}", public_error(&e))}),
            }
        }
        action => json!({"status": format!("ERROR: bad action '{action}'")}),
    };
    Json(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_are_clean_and_limited() {
        assert_eq!(parse_languages(" EN, de ,bad!,en "), vec!["de", "en"]);
        let many: String = (0..30).map(|i| format!("l{i},")).collect();
        assert_eq!(parse_languages(&many).len(), MAX_LANGUAGES);
    }

    #[test]
    fn sort_and_type_aliases() {
        assert_eq!(SortField::parse("ext_ids"), SortField::Identifiers);
        assert_eq!(SortField::parse("nonsense"), SortField::Score);
        assert_eq!(SearchType::parse("sites"), SearchType::Sites);
        assert_eq!(SearchType::parse("nonsense"), SearchType::Labels);
    }

    #[test]
    fn parameters_have_the_old_defaults() {
        let request = Request {
            action: None,
            r#type: String::new(),
            languages: "en".into(),
            present: String::new(),
            sparql: String::new(),
            sort_by: String::new(),
            sui: None,
            max_results: Some("1000".into()),
            offset: Some("-1".into()),
            mode: Some("GAME".into()),
        };
        assert_eq!(request.max_results(), MAX_RESULTS);
        assert_eq!(request.offset(), 0);
        assert_eq!(request.mode(), Mode::Game);
        assert_eq!(request.search_type(), SearchType::Labels);
    }
}
