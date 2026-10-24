//! `missingtopics` (formerly its own tool): the red links of an article, or
//! of the articles in a category tree, ranked by how often they are linked
//! to. For an article, links from all articles of the wiki count; for a tree,
//! links from the articles in the tree.

use crate::mediawiki;
use crate::replicas::{db_title, display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render, thousands};
use crate::wiki::Wiki;
use askama::Template;
use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const NS_ARTICLE: i64 = 0;
const NS_TEMPLATE: i64 = 10;
const DEFAULT_DEPTH: u32 = 1;
const MAX_DEPTH: u32 = 10;
/// Articles of a tree whose links are counted.
const MAX_PAGES: usize = 10_000;
/// Missing articles on the page and in wiki text; JSON has them all.
const MAX_SHOWN: usize = 5_000;
const LINKS_FROM: &str = "SELECT DISTINCT lt_title FROM pagelinks JOIN linktarget ON pl_target_id=lt_id
    WHERE lt_namespace=0 AND pl_from IN";

/// The old `wikimode` parameter.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
enum Output {
    #[default]
    #[serde(rename = "0")]
    Html,
    #[serde(rename = "1")]
    WikiTable,
    #[serde(rename = "2")]
    WikiList,
    #[serde(rename = "json")]
    Json,
}

const OUTPUTS: &[(Output, &str)] = &[
    (Output::Html, "Links"),
    (Output::WikiTable, "Wiki table"),
    (Output::WikiList, "Brief wiki list"),
    (Output::Json, "JSON"),
];

impl Output {
    fn code(self) -> &'static str {
        match self {
            Self::Html => "0",
            Self::WikiTable => "1",
            Self::WikiList => "2",
            Self::Json => "json",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    project: String,
    article: String,
    category: String,
    depth: Option<String>,
    wikimode: Output,
    /// Old links have `notemplatelinks=0` for "off".
    notemplatelinks: Option<String>,
    /// On if present, whatever the value; on by default in the empty form.
    nosingles: Option<String>,
    limitnum: Option<String>,
    doit: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            language: "de".to_string(),
            project: "wikipedia".to_string(),
            article: String::new(),
            category: String::new(),
            depth: None,
            wikimode: Output::Html,
            notemplatelinks: None,
            nosingles: None,
            limitnum: None,
            doit: None,
        }
    }
}

/// Where the links come from.
#[derive(Debug, PartialEq, Eq)]
enum Source {
    Article(String),
    Category { name: String, depth: u32 },
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    depth: u32,
    skip_template_links: bool,
    /// Only links wanted more than `limit` times.
    only_wanted: bool,
    limit: u64,
}

impl Options {
    fn new(request: &Request) -> Self {
        Self {
            depth: number(&request.depth).unwrap_or(DEFAULT_DEPTH).min(MAX_DEPTH),
            skip_template_links: request.notemplatelinks.as_deref().is_some_and(|v| !v.is_empty() && v != "0"),
            only_wanted: request.nosingles.is_some() || request.doit.is_none(),
            limit: number(&request.limitnum).unwrap_or(1),
        }
    }

    fn min_links(&self) -> u64 {
        if self.only_wanted { self.limit.saturating_add(1) } else { 1 }
    }
}

impl Source {
    /// An article, if given, beats a category.
    fn new(request: &Request, options: &Options) -> Option<Self> {
        let (article, category) = (request.article.trim(), request.category.trim());
        if !article.is_empty() {
            return Some(Self::Article(db_title(article)));
        }
        let category = mediawiki::strip_namespace(category, &["category", "kategorie"]).unwrap_or(category);
        (!category.is_empty()).then(|| Self::Category { name: db_title(category), depth: options.depth })
    }
}

struct Wanted {
    /// Database form.
    name: String,
    title: String,
    url: String,
    links_url: String,
    links: u64,
}

struct Results {
    /// Articles whose links were looked at.
    pages: usize,
    truncated: bool,
    template_links: usize,
    wanted: Vec<Wanted>,
}

impl Wanted {
    fn links_shown(&self) -> String {
        thousands(self.links)
    }
}

impl Results {
    fn shown(&self) -> &[Wanted] {
        &self.wanted[..self.wanted.len().min(MAX_SHOWN)]
    }
}

#[derive(Template)]
#[template(path = "tools/missingtopics.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    options: Options,
    outputs: &'static [(Output, &'static str)],
    results: Option<Results>,
    wikitext: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let options = Options::new(&request);
    let source = request.doit.as_ref().and_then(|_| Source::new(&request, &options));
    let Some(source) = source else {
        let page = Page {
            title: "Missing topics",
            request: &request,
            options,
            outputs: OUTPUTS,
            results: None,
            wikitext: None,
        };
        return Ok(render(&page)?.into_response());
    };
    let wiki = Wiki::new(&request.language, &request.project)?;
    let results = results(&state, &wiki, &source, &options).await?;
    let again = format!("{}/missingtopics/?{}", state.config.public_url, query(&request, &options));
    if request.wikimode == Output::Json {
        let ranked: Map<String, Value> = results.wanted.iter().map(|w| (w.name.clone(), w.links.into())).collect();
        return Ok(Json(json!({ "status": "OK", "results": ranked, "url": again })).into_response());
    }
    let wikitext = wikitext(request.wikimode, results.shown(), &again, &chrono::Utc::now().to_rfc2822());
    let page = Page {
        title: "Missing topics",
        request: &request,
        options,
        outputs: OUTPUTS,
        results: Some(results),
        wikitext,
    };
    Ok(render(&page)?.into_response())
}

async fn results(state: &AppState, wiki: &Wiki, source: &Source, options: &Options) -> Result<Results, AppError> {
    let min_links = options.min_links();
    let mut truncated = false;
    let (linking, mut counts) = match source {
        Source::Article(title) => {
            let ids = page_ids(state, wiki, NS_ARTICLE, std::slice::from_ref(title)).await?;
            if ids.is_empty() {
                return Err(AppError::BadRequest(format!("There is no article {} on {wiki}.", display_title(title))));
            }
            let mut conn = state.replicas.conn(wiki, &["pagelinks", "linktarget"]).await?;
            let links: Vec<String> = select_in(&mut conn, LINKS_FROM, "", &[], &ids).await?;
            let red = without_existing(state, wiki, links).await?;
            (ids, state.replicas.link_counts(wiki, &red, true).await?)
        }
        Source::Category { name, depth } => {
            let mut pages =
                state.replicas.some_pages_in_tree(wiki, name, *depth, NS_ARTICLE, Some(MAX_PAGES + 1)).await?;
            truncated = pages.len() > MAX_PAGES;
            pages.truncate(MAX_PAGES);
            let ids = page_ids(state, wiki, NS_ARTICLE, &pages).await?;
            let mut counts = counts_from(state, wiki, &ids).await?;
            // Fewer titles to check for existence.
            counts.retain(|_, n| *n >= min_links);
            let red: HashSet<String> =
                without_existing(state, wiki, counts.keys().cloned().collect()).await?.into_iter().collect();
            counts.retain(|title, _| red.contains(title));
            (ids, counts)
        }
    };
    counts.retain(|_, n| *n >= min_links);
    let mut template_links = 0;
    if options.skip_template_links {
        let in_templates = template_links_of(state, wiki, &linking).await?;
        let before = counts.len();
        counts.retain(|title, _| !in_templates.contains(title));
        template_links = before - counts.len();
    }
    let wanted = ranked(counts)
        .into_iter()
        .map(|(name, links)| Wanted {
            title: display_title(&name),
            url: wiki.page_url(&name),
            links_url: wiki.page_url(&format!("Special:WhatLinksHere/{name}")),
            name,
            links,
        })
        .collect();
    Ok(Results { pages: linking.len(), truncated, template_links, wanted })
}

/// Most links first, then by title.
fn ranked(counts: HashMap<String, u64>) -> Vec<(String, u64)> {
    let mut ranked: Vec<(String, u64)> = counts.into_iter().collect();
    ranked.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
    ranked
}

async fn page_ids(state: &AppState, wiki: &Wiki, namespace: i64, titles: &[String]) -> anyhow::Result<Vec<u64>> {
    let mut conn = state.replicas.conn(wiki, &["page"]).await?;
    let sql = "SELECT page_id FROM page WHERE page_namespace=? AND page_title IN";
    select_in(&mut conn, sql, "", &[namespace.into()], titles).await
}

async fn without_existing(state: &AppState, wiki: &Wiki, titles: Vec<String>) -> anyhow::Result<Vec<String>> {
    let existing = state.replicas.existing(wiki, NS_ARTICLE, &titles).await?;
    Ok(titles.into_iter().filter(|t| !existing.contains(t)).collect())
}

/// How many of the pages link to each article title.
async fn counts_from(state: &AppState, wiki: &Wiki, page_ids: &[u64]) -> anyhow::Result<HashMap<String, u64>> {
    let mut conn = state.replicas.conn(wiki, &["pagelinks", "linktarget"]).await?;
    let sql = "SELECT lt_title, COUNT(*) FROM pagelinks JOIN linktarget ON pl_target_id=lt_id
        WHERE lt_namespace=0 AND pl_from IN";
    let rows: Vec<(String, u64)> = select_in(&mut conn, sql, "GROUP BY lt_title", &[], page_ids).await?;
    let mut counts = HashMap::new();
    for (title, n) in rows {
        *counts.entry(title).or_default() += n;
    }
    Ok(counts)
}

/// A number parameter; anything else counts as absent, as in the old tool.
fn number<T: std::str::FromStr>(value: &Option<String>) -> Option<T> {
    value.as_deref().and_then(|s| s.trim().parse().ok())
}

/// Article titles linked from the templates the pages use.
async fn template_links_of(state: &AppState, wiki: &Wiki, pages: &[u64]) -> anyhow::Result<HashSet<String>> {
    let mut conn = state.replicas.conn(wiki, &["templatelinks", "pagelinks", "linktarget"]).await?;
    let sql = "SELECT DISTINCT lt_title FROM templatelinks JOIN linktarget ON tl_target_id=lt_id
        WHERE lt_namespace=10 AND tl_from IN";
    let templates: Vec<String> = select_in(&mut conn, sql, "", &[], pages).await?;
    let template_ids = page_ids(state, wiki, NS_TEMPLATE, &templates).await?;
    let links: Vec<String> = select_in(&mut conn, LINKS_FROM, "", &[], &template_ids).await?;
    Ok(links.into_iter().collect())
}

/// The query string that runs this again.
fn query(request: &Request, options: &Options) -> String {
    let (depth, limit) = (options.depth.to_string(), options.limit.to_string());
    let mut pairs = vec![
        ("language", request.language.trim()),
        ("project", request.project.trim()),
        ("article", request.article.trim()),
        ("category", request.category.trim()),
        ("depth", &depth),
        ("wikimode", request.wikimode.code()),
        ("limitnum", &limit),
    ];
    if options.skip_template_links {
        pairs.push(("notemplatelinks", "1"));
    }
    if options.only_wanted {
        pairs.push(("nosingles", "1"));
    }
    pairs.push(("doit", "1"));
    serde_urlencoded::to_string(pairs).unwrap_or_default()
}

/// The wiki text outputs, with a heading that links back here.
fn wikitext(output: Output, wanted: &[Wanted], again: &str, date: &str) -> Option<String> {
    let tool = again.split('?').next().unwrap_or(again);
    let mut text =
        format!("== Created {date} by [{tool} the MissingTopics tool] ([{again} run again with these settings]) ==\n");
    match output {
        Output::WikiTable => {
            text.push_str("{| class=\"wikitable sortable\"\n! Wanted !! Title\n");
            for w in wanted {
                text.push_str(&format!("|-\n| {} || [[{}]]\n", w.links, w.title));
            }
            text.push_str("|}\n");
        }
        Output::WikiList => {
            for w in wanted {
                text.push_str(&format!("[[{}]] -\n", w.title));
            }
        }
        Output::Html | Output::Json => return None,
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    fn request(query: &str) -> Request {
        serde_urlencoded::from_str(query).unwrap()
    }

    fn wanted(title: &str, links: u64) -> Wanted {
        Wanted {
            name: title.replace(' ', "_"),
            title: title.to_string(),
            url: String::new(),
            links_url: String::new(),
            links,
        }
    }

    #[test]
    fn options_as_the_old_tool_read_them() {
        let empty = Options::new(&request(""));
        assert_eq!(empty, Options { depth: 1, skip_template_links: false, only_wanted: true, limit: 1 });
        let run = Options::new(&request("doit=Run&depth=99&limitnum=x&notemplatelinks=0"));
        assert_eq!(run, Options { depth: MAX_DEPTH, skip_template_links: false, only_wanted: false, limit: 1 });
        assert_eq!(run.min_links(), 1);
        let wanted = Options::new(&request("doit=1&nosingles=1&limitnum=3&notemplatelinks=1"));
        assert!(wanted.skip_template_links);
        assert_eq!(wanted.min_links(), 4);
    }

    #[test]
    fn sources() {
        let source = |q: &str| Source::new(&request(q), &Options::new(&request(q)));
        assert_eq!(source("article=foo bar&category=X"), Some(Source::Article("Foo_bar".into())));
        assert_eq!(
            source("category=Kategorie:Leuchtturm in Schottland&depth=2"),
            Some(Source::Category { name: "Leuchtturm_in_Schottland".into(), depth: 2 })
        );
        assert_eq!(source("article= &category="), None);
    }

    #[test]
    fn outputs_parse_and_round_trip() {
        assert_eq!(request("wikimode=json").wikimode, Output::Json);
        assert_eq!(request("wikimode=2").wikimode, Output::WikiList);
        assert!(serde_urlencoded::from_str::<Request>("wikimode=7").is_err());
        let r = request("language=en&category=A b&wikimode=1&doit=1");
        assert_eq!(
            query(&r, &Options::new(&r)),
            "language=en&project=wikipedia&article=&category=A+b&depth=1&wikimode=1&limitnum=1&doit=1"
        );
    }

    #[test]
    fn ranking() {
        let counts = HashMap::from([("B".to_string(), 2), ("A".to_string(), 2), ("C".to_string(), 5)]);
        let names: Vec<String> = ranked(counts).into_iter().map(|(t, _)| t).collect();
        assert_eq!(names, ["C", "A", "B"]);
    }

    #[test]
    fn wiki_text() {
        let list = [wanted("Foo bar", 3)];
        let again = "https://x.org/missingtopics/?a=1";
        let table = wikitext(Output::WikiTable, &list, again, "D").unwrap();
        assert!(table.starts_with("== Created D by [https://x.org/missingtopics/ the MissingTopics tool]"));
        assert!(table.contains("|-\n| 3 || [[Foo bar]]\n|}"));
        assert!(wikitext(Output::WikiList, &list, again, "D").unwrap().ends_with("[[Foo bar]] -\n"));
        assert!(wikitext(Output::Html, &list, again, "D").is_none());
    }

    #[tokio::test]
    async fn form_and_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let form = get(dir.path(), "/missingtopics/").await;
        assert_eq!(form.status.as_u16(), 200);
        assert!(form.body.contains(r#"name="nosingles" value="1" checked"#));
        let unknown = get(dir.path(), "/missingtopics/?language=xx.evil&article=A&doit=1").await;
        assert_eq!(unknown.status.as_u16(), 400);
        let old = get(dir.path(), "/missingtopics.php?article=A").await;
        assert_eq!(old.headers["location"], "/missingtopics/?article=A");
    }
}
