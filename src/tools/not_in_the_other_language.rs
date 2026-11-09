//! `not-in-the-other-language.php`: Wikidata items with a page on one wiki
//! but not on another, optionally limited to a category tree or a PagePile.

use crate::replicas::{display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, upper_first};
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use pagepile::client::{Client, DEFAULT_API, NewPage, Query as PagePileQuery};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Params as DbParams, Value};

const DEFAULT_LANGUAGE1: &str = "de";
const DEFAULT_LANGUAGE2: &str = "en";
const DEFAULT_PROJECT: &str = "wiki";
const DEFAULT_DEPTH: u32 = 9;
const MAX_DEPTH: u32 = 20;
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1_000;
/// Pages of the category tree resolved into the `IN` list.
const MAX_CATEGORY_PAGES: usize = 50_000;
/// Pages read from a PagePile input.
const MAX_PILE_PAGES: usize = 100_000;
/// Results kept in memory, and in a PagePile we create.
const MAX_OUTPUT: usize = 100_000;

const PROJECTS: &[(&str, &str)] =
    &[("wiki", "Wikipedia"), ("wikisource", "Wikisource"), ("wikivoyage", "Wikivoyage"), ("wiktionary", "Wiktionary")];

/// Content language for Wikidata labels; these wikis' codes differ.
fn content_language(language: &str) -> &str {
    match language {
        "als" => "gsw",
        "bat-smg" => "sgs",
        "be-x-old" => "be-tarask",
        "bh" => "bho",
        "crh" => "chr-latn",
        "fiu-vro" => "vro",
        "no" => "nb",
        "roa-rup" => "rup",
        "simple" => "en",
        "zh-classical" => "lzh",
        "zh-min-nan" => "nan",
        "zh-yue" => "yue",
        language => language,
    }
}

fn text_or(value: &str, default: &str) -> String {
    let value = value.trim();
    match value.is_empty() {
        true => default.to_string(),
        false => value.to_lowercase(),
    }
}

fn project_family(project: &str) -> Result<&'static str, AppError> {
    match project {
        "" | "wiki" => Ok("wikipedia"),
        "wikisource" => Ok("wikisource"),
        "wikivoyage" => Ok("wikivoyage"),
        "wiktionary" => Ok("wiktionary"),
        other => Err(AppError::BadRequest(format!("“{other}” is not one of the projects this tool knows."))),
    }
}

/// `Category:Some name` => `Some_name`, as the database has it.
fn category_title(input: &str) -> String {
    let input = input.trim();
    let without_prefix = match input.get(..9) {
        Some(prefix) if prefix.eq_ignore_ascii_case("category:") => input.get(9..).unwrap_or_default(),
        _ => input,
    };
    upper_first(&without_prefix.replace(' ', "_"))
}

fn parse_start(value: Option<&str>) -> usize {
    value.and_then(|value| value.trim().parse().ok()).unwrap_or(0)
}

fn parse_limit(value: Option<&str>) -> usize {
    value.and_then(|value| value.trim().parse().ok()).unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn parse_depth(value: Option<&str>) -> u32 {
    value.and_then(|value| value.trim().parse().ok()).unwrap_or(DEFAULT_DEPTH).min(MAX_DEPTH)
}

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    lang1: String,
    #[serde(default)]
    proj1: String,
    #[serde(default)]
    lang2: String,
    #[serde(default)]
    proj2: String,
    #[serde(default)]
    cat: String,
    depth: Option<String>,
    #[serde(default)]
    starts_with: String,
    #[serde(default)]
    targets: String,
    #[serde(default)]
    format: String,
    pagepile: Option<String>,
    start: Option<String>,
    limit: Option<String>,
    doit: Option<String>,
}

struct Form {
    lang1: String,
    proj1: String,
    lang2: String,
    proj2: String,
    cat: String,
    depth: u32,
    starts_with: String,
    targets: String,
    format: String,
    pagepile: String,
}

struct DisplayRow {
    label: String,
    url: String,
}

struct Results {
    rows: Vec<DisplayRow>,
    first: usize,
    last: usize,
    category_truncated: bool,
    pile_truncated: bool,
    capped: bool,
    no_pile: bool,
}

#[derive(Default)]
struct Pager {
    prev: Option<String>,
    next: Option<String>,
}

#[derive(Template)]
#[template(path = "tools/not_in_the_other_language.html")]
struct Page {
    title: &'static str,
    form: Form,
    projects: &'static [(&'static str, &'static str)],
    results: Option<Results>,
    pager: Pager,
}

#[derive(Debug, Clone)]
struct Match {
    item: u64,
    page: String,
}

struct Fetch {
    rows: Vec<Match>,
    has_next: bool,
    capped: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Wikidata,
    Source,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let lang1 = text_or(&request.lang1, DEFAULT_LANGUAGE1);
    let lang2 = text_or(&request.lang2, DEFAULT_LANGUAGE2);
    let proj1 = match request.proj1.trim() {
        "" => DEFAULT_PROJECT.to_string(),
        proj => proj.to_lowercase(),
    };
    let proj2 = match request.proj2.trim() {
        "" => DEFAULT_PROJECT.to_string(),
        proj => proj.to_lowercase(),
    };
    let category = category_title(&request.cat);
    let depth = parse_depth(request.depth.as_deref());
    let start = parse_start(request.start.as_deref());
    let limit = parse_limit(request.limit.as_deref());
    let targets = match request.targets.trim().eq_ignore_ascii_case("source") {
        true => "source".to_string(),
        false => "wikidata".to_string(),
    };
    let format = match request.format.trim().eq_ignore_ascii_case("pagepile") {
        true => "pagepile".to_string(),
        false => "html".to_string(),
    };
    let pagepile = request.pagepile.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(str::to_string);
    let pagepile_id = pagepile
        .as_deref()
        .map(|p| p.parse::<u64>())
        .transpose()
        .map_err(|_| AppError::BadRequest("The PagePile parameter must be a pile ID.".to_string()))?;
    let form = Form {
        lang1,
        proj1,
        lang2,
        proj2,
        cat: display_title(&category),
        depth,
        starts_with: request.starts_with.trim().to_string(),
        targets,
        format,
        pagepile: pagepile.unwrap_or_default(),
    };

    let mut results = None;
    let mut pagination = Pager::default();
    if request.doit.is_some() {
        let wiki1 = Wiki::new(&form.lang1, project_family(&form.proj1)?)?;
        let wiki2 = Wiki::new(&form.lang2, project_family(&form.proj2)?)?;
        let target = match form.targets.as_str() {
            "source" => Target::Source,
            _ => Target::Wikidata,
        };
        let output_pagepile = form.format == "pagepile";

        let (category_pages, category_truncated) = match category.is_empty() {
            true => (None, false),
            false => {
                let (pages, truncated) = category_pages(&state, &wiki1, &category, depth).await?;
                (Some(pages), truncated)
            }
        };
        let (pile_items, pile_truncated) = match pagepile_id {
            Some(id) => {
                let (items, truncated) = pagepile_items(&state, id).await?;
                (Some(items), truncated)
            }
            None => (None, false),
        };
        let starts_with = match form.starts_with.is_empty() {
            true => None,
            false => Some(form.starts_with.replace('_', " ")),
        };

        let fetch = fetch_matches(
            &state,
            wiki1.dbname(),
            wiki2.dbname(),
            category_pages.as_deref(),
            pile_items.as_deref(),
            starts_with.as_deref(),
            start,
            limit,
            output_pagepile,
        )
        .await?;

        if output_pagepile && !fetch.rows.is_empty() {
            let wiki = match target {
                Target::Wikidata => Wiki::wikidata(),
                Target::Source => wiki1.clone(),
            };
            let url = create_pile(&state, &wiki, target, &fetch.rows).await?;
            return Ok(Redirect::to(&url).into_response());
        }

        let rows: Vec<DisplayRow> = match target {
            Target::Source => fetch
                .rows
                .iter()
                .map(|row| DisplayRow { label: row.page.clone(), url: wiki1.page_url(&row.page) })
                .collect(),
            Target::Wikidata => {
                let ids: Vec<u64> = fetch.rows.iter().map(|row| row.item).collect();
                let labels = labels(&state, &ids, content_language(&form.lang1)).await?;
                fetch
                    .rows
                    .iter()
                    .map(|row| DisplayRow {
                        label: labels.get(&row.item).cloned().unwrap_or_else(|| format!("Q{}", row.item)),
                        url: Wiki::wikidata().page_url(&format!("Q{}", row.item)),
                    })
                    .collect()
            }
        };
        let first = match rows.is_empty() {
            true => 0,
            false => start + 1,
        };
        let last = start + rows.len();
        results = Some(Results {
            rows,
            first,
            last,
            category_truncated,
            pile_truncated,
            capped: fetch.capped,
            no_pile: output_pagepile,
        });
        if !output_pagepile {
            pagination = make_pager(&form, start, limit, fetch.has_next);
        }
    }

    let page = Page { title: "Not in the other language", form, projects: PROJECTS, results, pager: pagination };
    Ok(render(&page)?.into_response())
}

async fn category_pages(
    state: &AppState,
    wiki: &Wiki,
    category: &str,
    depth: u32,
) -> Result<(Vec<String>, bool), AppError> {
    let pages = state.replicas.pages_in_tree(wiki, category, depth, 0).await?;
    let mut pages: Vec<String> = pages.into_iter().map(|page| display_title(&page)).collect();
    pages.sort();
    pages.dedup();
    let truncated = pages.len() > MAX_CATEGORY_PAGES;
    pages.truncate(MAX_CATEGORY_PAGES);
    Ok((pages, truncated))
}

async fn pagepile_items(state: &AppState, id: u64) -> Result<(Vec<u64>, bool), AppError> {
    let client = Client::with_http(state.http.clone(), DEFAULT_API);
    let limit = (MAX_PILE_PAGES + 1) as u64;
    let pile = client
        .pile_with(id, &PagePileQuery::default().limit(limit))
        .await
        .map_err(|e| AppError::Upstream(anyhow::Error::new(e)))?;
    let truncated = pile.pages_total > MAX_PILE_PAGES as u64;

    if pile.wiki == Wiki::wikidata().dbname() {
        let mut items: Vec<u64> =
            pile.items().filter_map(|item| item.strip_prefix('Q').and_then(|number| number.parse().ok())).collect();
        items.sort_unstable();
        items.dedup();
        items.truncate(MAX_PILE_PAGES);
        return Ok((items, truncated));
    }

    // A pile on a source wiki: map its pages to Wikidata items by namespace and
    // title. `page_props` avoids the localized namespace prefixes.
    let mut by_namespace: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for page in pile.pages.iter().take(MAX_PILE_PAGES) {
        by_namespace.entry(page.namespace).or_default().push(page.page.clone());
    }
    let mut conn = state.replicas.conn_for_dbname(&pile.wiki, &["page", "page_props"]).await?;
    let sql = "SELECT pp_value FROM page JOIN page_props ON pp_page=page_id
        WHERE page_namespace=? AND pp_propname='wikibase_item' AND page_title IN";
    let mut items = vec![];
    for (namespace, pages) in by_namespace {
        let values: Vec<String> = select_in(&mut conn, sql, "", &[Value::from(namespace)], &pages).await?;
        items.extend(
            values.into_iter().filter_map(|item| item.strip_prefix('Q').and_then(|number| number.parse::<u64>().ok())),
        );
    }
    items.sort_unstable();
    items.dedup();
    items.truncate(MAX_PILE_PAGES);
    Ok((items, truncated))
}

#[allow(clippy::too_many_arguments)]
async fn fetch_matches(
    state: &AppState,
    site1: &str,
    site2: &str,
    category_pages: Option<&[String]>,
    pile_items: Option<&[u64]>,
    starts_with: Option<&str>,
    start: usize,
    limit: usize,
    all: bool,
) -> Result<Fetch, AppError> {
    let mut conn = state.replicas.conn(&Wiki::wikidata(), &["wb_items_per_site"]).await?;
    let base = "SELECT i1.ips_item_id, i1.ips_site_page FROM wb_items_per_site i1
        LEFT OUTER JOIN wb_items_per_site i2 ON i1.ips_item_id=i2.ips_item_id AND i2.ips_site_id=?
        WHERE i1.ips_site_id=? AND i2.ips_site_id IS NULL";
    let mut params = vec![Value::from(site2), Value::from(site1)];
    let starts_filter = match starts_with {
        Some(starts_with) => {
            params.push(Value::from(starts_with));
            " AND i1.ips_site_page >= ?"
        }
        None => "",
    };

    let filtered = category_pages.is_some() || pile_items.is_some();
    if !filtered {
        let take = if all { MAX_OUTPUT } else { limit };
        let sql = format!("{base}{starts_filter} ORDER BY i1.ips_site_page LIMIT ? OFFSET ?");
        params.push(Value::from((take + 1) as u64));
        params.push(Value::from(if all { 0 } else { start } as u64));
        let rows: Vec<(u64, String)> = conn
            .exec(sql, DbParams::Positional(params))
            .await
            .map_err(|e| AppError::Upstream(anyhow::Error::new(e)))?;
        let mut rows: Vec<Match> = rows.into_iter().map(|(item, page)| Match { item, page }).collect();
        let has_next = rows.len() > take;
        rows.truncate(take);
        return Ok(Fetch { rows, has_next, capped: false });
    }

    if category_pages.is_some_and(|pages| pages.is_empty()) || pile_items.is_some_and(|items| items.is_empty()) {
        return Ok(Fetch { rows: vec![], has_next: false, capped: false });
    }
    let mut rows = match (category_pages, pile_items) {
        (Some(category_pages), pile_items) => {
            let sql = format!("{base}{starts_filter} AND i1.ips_site_page IN");
            let rows: Vec<(u64, String)> = select_in(&mut conn, &sql, "", &params, category_pages).await?;
            let mut rows: Vec<Match> = rows.into_iter().map(|(item, page)| Match { item, page }).collect();
            if let Some(pile_items) = pile_items {
                let pile_items: HashSet<u64> = pile_items.iter().copied().collect();
                rows.retain(|row| pile_items.contains(&row.item));
            }
            rows
        }
        (None, Some(pile_items)) => {
            let sql = format!("{base}{starts_filter} AND i1.ips_item_id IN");
            let rows: Vec<(u64, String)> = select_in(&mut conn, &sql, "", &params, pile_items).await?;
            rows.into_iter().map(|(item, page)| Match { item, page }).collect()
        }
        (None, None) => vec![],
    };
    rows.sort_by(|a, b| a.page.cmp(&b.page).then(a.item.cmp(&b.item)));
    rows.dedup_by(|a, b| a.item == b.item);
    let total = rows.len();
    let capped = total > MAX_OUTPUT;
    rows.truncate(MAX_OUTPUT);
    let has_next = !all && total > start + limit;
    let rows = if all { rows } else { rows.into_iter().skip(start).take(limit).collect() };
    Ok(Fetch { rows, has_next, capped })
}

async fn labels(state: &AppState, ids: &[u64], language: &str) -> Result<HashMap<u64, String>, AppError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let api = state.api(&Wiki::wikidata()).await?;
    let mut labels = HashMap::new();
    for chunk in ids.chunks(50) {
        let ids = chunk.iter().map(|id| format!("Q{id}")).collect::<Vec<_>>().join("|");
        let languages = format!("{language}|en|mul");
        let params = api.params_into(&[
            ("action", "wbgetentities"),
            ("ids", &ids),
            ("props", "labels"),
            ("languages", &languages),
            ("languagefallback", "1"),
        ]);
        let json = api.query_api_json(&params, "GET").await?;
        for (qid, entity) in json["entities"].as_object().into_iter().flatten() {
            let label = ["language", "en", "mul"]
                .iter()
                .find_map(|language| entity["labels"][*language]["value"].as_str())
                .unwrap_or(qid);
            if let Some(id) = qid.strip_prefix('Q').and_then(|number| number.parse().ok()) {
                labels.insert(id, label.to_string());
            }
        }
    }
    Ok(labels)
}

async fn create_pile(state: &AppState, wiki: &Wiki, target: Target, rows: &[Match]) -> Result<String, AppError> {
    let pages: Vec<NewPage> = match target {
        Target::Wikidata => rows.iter().map(|row| NewPage::new(format!("Q{}", row.item), 0)).collect(),
        Target::Source => rows.iter().map(|row| NewPage::title(row.page.clone())).collect(),
    };
    let client = Client::with_http(state.http.clone(), DEFAULT_API);
    let id = client.create(wiki.dbname(), &pages).await.map_err(|e| AppError::Upstream(anyhow::Error::new(e)))?;
    Ok(client.view_url(id))
}

fn make_pager(form: &Form, start: usize, limit: usize, has_next: bool) -> Pager {
    let link = |start: usize| {
        let mut pairs: Vec<(&str, String)> = vec![
            ("lang1", form.lang1.clone()),
            ("proj1", form.proj1.clone()),
            ("lang2", form.lang2.clone()),
            ("proj2", form.proj2.clone()),
            ("cat", form.cat.clone()),
            ("depth", form.depth.to_string()),
            ("starts_with", form.starts_with.clone()),
            ("targets", form.targets.clone()),
            ("format", form.format.clone()),
            ("start", start.to_string()),
            ("limit", limit.to_string()),
            ("doit", "1".to_string()),
        ];
        if !form.pagepile.is_empty() {
            pairs.push(("pagepile", form.pagepile.clone()));
        }
        let query = serde_urlencoded::to_string(pairs).unwrap_or_default();
        format!("/not-in-the-other-language/?{query}")
    };
    Pager { prev: (start > 0).then(|| link(start.saturating_sub(limit))), next: has_next.then(|| link(start + limit)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_languages() {
        assert_eq!(content_language("de"), "de");
        assert_eq!(content_language("simple"), "en");
        assert_eq!(content_language("be-x-old"), "be-tarask");
    }

    #[test]
    fn categories_and_projects() {
        assert_eq!(category_title(" Category:Foo Bar "), "Foo_Bar");
        assert_eq!(project_family("wiki").unwrap(), "wikipedia");
        assert!(project_family("commons").is_err());
    }

    #[test]
    fn paging_values() {
        assert_eq!(parse_start(None), 0);
        assert_eq!(parse_start(Some("20")), 20);
        assert_eq!(parse_limit(None), DEFAULT_LIMIT);
        assert_eq!(parse_limit(Some("0")), 1);
        assert_eq!(parse_limit(Some("9999")), MAX_LIMIT);
        assert_eq!(parse_depth(Some("99")), MAX_DEPTH);
    }

    #[test]
    fn form_renders() {
        let page = Page {
            title: "Not in the other language",
            form: Form {
                lang1: "de".into(),
                proj1: "wiki".into(),
                lang2: "en".into(),
                proj2: "wiki".into(),
                cat: String::new(),
                depth: DEFAULT_DEPTH,
                starts_with: String::new(),
                targets: "wikidata".into(),
                format: "html".into(),
                pagepile: String::new(),
            },
            projects: PROJECTS,
            results: None,
            pager: Pager::default(),
        };
        let html = page.render().unwrap();
        assert!(html.contains("<h1>Not in the other language</h1>"));
        assert!(html.contains("name=\"proj1\""));
        assert!(html.contains("name=\"doit\""));
    }

    #[test]
    fn result_and_pager_render() {
        let page = Page {
            title: "Not in the other language",
            form: Form {
                lang1: "de".into(),
                proj1: "wiki".into(),
                lang2: "en".into(),
                proj2: "wiki".into(),
                cat: String::new(),
                depth: DEFAULT_DEPTH,
                starts_with: String::new(),
                targets: "wikidata".into(),
                format: "html".into(),
                pagepile: String::new(),
            },
            projects: PROJECTS,
            results: Some(Results {
                rows: vec![DisplayRow { label: "Example".into(), url: "https://www.wikidata.org/wiki/Q1".into() }],
                first: 1,
                last: 1,
                category_truncated: false,
                pile_truncated: false,
                capped: false,
                no_pile: false,
            }),
            pager: Pager { prev: None, next: Some("/not-in-the-other-language/?lang1=de&start=100&doit=1".into()) },
        };
        let html = page.render().unwrap();
        assert!(html.contains("https://www.wikidata.org/wiki/Q1"));
        assert!(html.contains("start=100"));
        assert!(html.contains("Results 1–1"));
    }
}
