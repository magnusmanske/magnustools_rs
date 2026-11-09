//! `missingtopics` (formerly its own tool): the red links of an article, or
//! of the articles in a category tree, ranked by how often they are linked
//! to. For an article, links from all articles of the wiki count; for a tree,
//! links from the articles in the tree.

use crate::mediawiki;
use crate::replicas::{db_title, display_title, placeholders, select_in};
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
use std::time::{Duration, Instant};
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Params as DbParams, Value as DbValue};

const NS_ARTICLE: i64 = 0;
const NS_TEMPLATE: i64 = 10;
const DEFAULT_DEPTH: u32 = 1;
/// As deep as the old tool went; categories can nest that far.
const MAX_DEPTH: u32 = 30;
/// Articles of a tree whose links are counted; a huge tree must not use unbounded memory.
/// Large portals (e.g. dewiki "Literatur") have a few hundred thousand articles.
const MAX_PAGES: usize = 400_000;
/// Missing articles kept (the most wanted); wiki text and JSON may show this many.
const MAX_RESULTS: usize = 50_000;
/// Time for the replica work before returning what is done. The request
/// timeout is 300 s, so leave room for the existence and template checks.
const QUERY_BUDGET: Duration = Duration::from_secs(200);
/// Missing articles in the HTML tables.
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
    #[serde(rename = "3")]
    WikiListByDemand,
    #[serde(rename = "json")]
    Json,
}

const OUTPUTS: &[(Output, &str)] = &[
    (Output::Html, "Links"),
    (Output::WikiTable, "Wiki table"),
    (Output::WikiList, "Brief wiki list (alphabetical)"),
    (Output::WikiListByDemand, "Brief wiki list (by demand)"),
    (Output::Json, "JSON"),
];

impl Output {
    fn code(self) -> &'static str {
        match self {
            Self::Html => "0",
            Self::WikiTable => "1",
            Self::WikiList => "2",
            Self::WikiListByDemand => "3",
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

impl Request {
    /// Links the old tool left in wiki pages have the settings but no `doit`.
    fn wants_results(&self) -> bool {
        self.doit.is_some() || !self.article.trim().is_empty() || !self.category.trim().is_empty()
    }
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
    /// Page ID; the page can be in any namespace.
    Article(u64),
    Category {
        name: String,
        depth: u32,
    },
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
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
            only_wanted: request.nosingles.is_some() || !request.wants_results(),
            limit: number(&request.limitnum).unwrap_or(1),
        }
    }

    fn min_links(&self) -> u64 {
        if self.only_wanted { self.limit.saturating_add(1) } else { 1 }
    }
}

impl Source {
    /// An article, if given, beats a category.
    async fn new(
        state: &AppState,
        wiki: &Wiki,
        request: &Request,
        options: &Options,
    ) -> Result<Option<Self>, AppError> {
        let article = request.article.trim();
        if !article.is_empty() {
            let page = mediawiki::resolve_pages(&state.http, wiki, &[article])
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| AppError::BadRequest(format!("There is no page {article} on {wiki}.")))?;
            return Ok(Some(Self::Article(page.0)));
        }
        Ok(Self::category(request, options))
    }

    fn category(request: &Request, options: &Options) -> Option<Self> {
        let category = request.category.trim();
        let category = mediawiki::strip_namespace(category, &["category", "kategorie"]).unwrap_or(category);
        (!category.is_empty()).then(|| Self::Category { name: db_title(category), depth: options.depth })
    }
}

#[derive(Clone)]
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
    /// The category tree had more articles than [`MAX_PAGES`].
    pages_truncated: bool,
    /// More than [`MAX_RESULTS`] missing articles were found.
    results_truncated: bool,
    template_links: usize,
    /// Most wanted first.
    wanted: Vec<Wanted>,
    /// All entries, by title.
    alphabetical: Vec<Wanted>,
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

    /// As many alphabetically sorted entries as the HTML table shows.
    fn shown_alphabetical(&self) -> &[Wanted] {
        &self.alphabetical[..self.alphabetical.len().min(MAX_SHOWN)]
    }

    /// The entries in the order the output wants them.
    fn for_output(&self, output: Output) -> &[Wanted] {
        match output {
            Output::WikiList => &self.alphabetical,
            _ => &self.wanted,
        }
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

fn form(request: &Request, options: Options) -> Page<'_> {
    Page { title: "Missing topics", request, options, outputs: OUTPUTS, results: None, wikitext: None }
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let options = Options::new(&request);
    if !request.wants_results() {
        return Ok(render(&form(&request, options))?.into_response());
    }
    let wiki = Wiki::new(&request.language, &request.project)?;
    let Some(source) = Source::new(&state, &wiki, &request, &options).await? else {
        return Ok(render(&form(&request, options))?.into_response());
    };
    let results = results(&state, &wiki, &source, &options).await?;
    let again = format!("{}/missingtopics/?{}", state.config.public_url, query(&request, &options));
    if request.wikimode == Output::Json {
        let ranked: Map<String, Value> = results.wanted.iter().map(|w| (w.name.clone(), w.links.into())).collect();
        return Ok(Json(json!({ "status": "OK", "results": ranked, "url": again })).into_response());
    }
    let wikitext =
        wikitext(request.wikimode, results.for_output(request.wikimode), &again, &chrono::Utc::now().to_rfc2822());
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
    let deadline = Instant::now() + QUERY_BUDGET;
    let (linking, mut counts, pages_truncated, mut results_truncated) = match source {
        Source::Article(id) => {
            let mut conn = state.replicas.conn(wiki, &["pagelinks", "linktarget"]).await?;
            let links: Vec<String> = select_in(&mut conn, LINKS_FROM, "", &[], std::slice::from_ref(id)).await?;
            let red = without_existing(state, wiki, links).await?;
            (vec![*id], state.replicas.link_counts(wiki, &red, true).await?, false, false)
        }
        Source::Category { name, depth } => {
            let (ids, truncated) = page_ids_in_tree(state, wiki, name, *depth, deadline).await?;
            let (counts, results_truncated) = counts_from(state, wiki, &ids, MAX_RESULTS, deadline).await?;
            (ids, counts, truncated, results_truncated)
        }
    };
    // Fewer titles to check for existence.
    counts.retain(|_, n| *n >= min_links);
    let red: HashSet<String> =
        without_existing(state, wiki, counts.keys().cloned().collect()).await?.into_iter().collect();
    counts.retain(|title, _| red.contains(title));
    counts.retain(|_, n| *n >= min_links);
    let mut template_links = 0;
    if options.skip_template_links {
        let in_templates = template_links_of(state, wiki, &linking).await?;
        let before = counts.len();
        counts.retain(|title, _| !in_templates.contains(title));
        template_links = before - counts.len();
    }
    let mut wanted: Vec<Wanted> = ranked(counts)
        .into_iter()
        .map(|(name, links)| Wanted {
            title: display_title(&name),
            url: wiki.page_url(&name),
            links_url: wiki.page_url(&format!("Special:WhatLinksHere/{name}")),
            name,
            links,
        })
        .collect();
    if wanted.len() > MAX_RESULTS {
        wanted.truncate(MAX_RESULTS);
        results_truncated = true;
    }
    let alphabetical = alphabetical(&wanted);
    if (pages_truncated || results_truncated)
        && let Source::Category { name, depth } = source
    {
        tracing::info!(
            "missingtopics: {name} (depth {depth}): {} pages, {} results (truncated)",
            linking.len(),
            wanted.len()
        );
    }
    Ok(Results { pages: linking.len(), pages_truncated, results_truncated, template_links, wanted, alphabetical })
}

/// Most links first, then by title.
fn ranked(counts: HashMap<String, u64>) -> Vec<(String, u64)> {
    let mut ranked: Vec<(String, u64)> = counts.into_iter().collect();
    ranked.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
    ranked
}

/// All entries, by title.
fn alphabetical(wanted: &[Wanted]) -> Vec<Wanted> {
    let mut alphabetical = wanted.to_vec();
    alphabetical.sort_by(|a, b| a.title.cmp(&b.title));
    alphabetical
}

/// Article page IDs in the category tree, at most [`MAX_PAGES`]; the flag says
/// some were left out. Stops when the query budget is spent.
async fn page_ids_in_tree(
    state: &AppState,
    wiki: &Wiki,
    category: &str,
    depth: u32,
    deadline: Instant,
) -> anyhow::Result<(Vec<u64>, bool)> {
    let categories = state.replicas.category_tree(wiki, category, depth).await?;
    let sql = "SELECT DISTINCT page_id FROM page
        JOIN categorylinks ON cl_from=page_id JOIN linktarget ON cl_target_id=lt_id
        WHERE page_namespace=? AND lt_namespace=14 AND lt_title IN";
    let mut conn = state.replicas.conn(wiki, &["page", "categorylinks", "linktarget"]).await?;
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    let mut truncated = false;
    for chunk in categories.chunks(1000) {
        if Instant::now() >= deadline {
            truncated = true;
            break;
        }
        let remaining = MAX_PAGES + 1 - ids.len();
        let suffix = format!("LIMIT {remaining}");
        let rows: Vec<u64> = select_in(&mut conn, sql, &suffix, &[NS_ARTICLE.into()], chunk).await?;
        ids.extend(rows.into_iter().filter(|id| seen.insert(*id)));
        if ids.len() > MAX_PAGES {
            ids.truncate(MAX_PAGES);
            truncated = true;
            break;
        }
    }
    ids.sort_unstable();
    Ok((ids, truncated))
}

async fn page_ids(state: &AppState, wiki: &Wiki, namespace: i64, titles: &[String]) -> anyhow::Result<Vec<u64>> {
    let mut conn = state.replicas.conn(wiki, &["page"]).await?;
    let sql = "SELECT page_id FROM page WHERE page_namespace=? AND page_title IN";
    select_in(&mut conn, sql, "", &[namespace.into()], titles).await.map_err(anyhow::Error::from)
}

async fn without_existing(state: &AppState, wiki: &Wiki, titles: Vec<String>) -> anyhow::Result<Vec<String>> {
    let existing = state.replicas.existing(wiki, NS_ARTICLE, &titles).await?;
    Ok(titles.into_iter().filter(|t| !existing.contains(t)).collect())
}

/// How many of the pages link to each article title, keeping the `limit` most
/// wanted titles; the flag says more were found. The limit is applied per
/// chunk of pages, and the query budget stops very large trees.
async fn counts_from(
    state: &AppState,
    wiki: &Wiki,
    page_ids: &[u64],
    limit: usize,
    deadline: Instant,
) -> anyhow::Result<(HashMap<String, u64>, bool)> {
    let mut conn = state.replicas.conn(wiki, &["pagelinks", "linktarget"]).await?;
    let mut counts = HashMap::new();
    let mut truncated = false;
    for chunk in page_ids.chunks(5000) {
        if Instant::now() >= deadline {
            truncated = true;
            break;
        }
        let sql = format!(
            "SELECT lt_title, COUNT(*) FROM pagelinks JOIN linktarget ON pl_target_id=lt_id
             WHERE lt_namespace=0 AND pl_from IN ({}) GROUP BY lt_title
             ORDER BY COUNT(*) DESC LIMIT {limit}",
            placeholders(chunk.len())
        );
        let params: Vec<DbValue> = chunk.iter().map(|id| DbValue::from(*id)).collect();
        let rows: Vec<(String, u64)> = conn.exec(sql, DbParams::Positional(params)).await?;
        if rows.len() >= limit {
            truncated = true;
        }
        for (title, n) in rows {
            *counts.entry(title).or_default() += n;
        }
        if counts.len() > limit {
            trim_top(&mut counts, limit);
            truncated = true;
        }
    }
    if counts.len() >= limit {
        truncated = true;
    }
    Ok((counts, truncated))
}

/// Keep the `limit` titles with the most links.
fn trim_top(counts: &mut HashMap<String, u64>, limit: usize) {
    let mut ranked: Vec<(String, u64)> = std::mem::take(counts).into_iter().collect();
    ranked.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
    ranked.truncate(limit);
    *counts = ranked.into_iter().collect();
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
        Output::WikiList | Output::WikiListByDemand => {
            // One title per line, so the source stays readable when pasted
            // into a portal page.
            for w in wanted {
                text.push_str(&format!("[[{}]] –\n", w.title));
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
    fn old_run_again_links_run() {
        assert!(!request("").wants_results());
        assert!(!request("language=de").wants_results());
        assert!(request("doit=1").wants_results());
        assert!(request("category=X&nosingles=1").wants_results());
        assert!(request("article=Vorlage:X&wikimode=2").wants_results());
    }

    #[test]
    fn categories() {
        let source = |q: &str| Source::category(&request(q), &Options::new(&request(q)));
        assert_eq!(
            source("category=Kategorie:Leuchtturm in Schottland&depth=2"),
            Some(Source::Category { name: "Leuchtturm_in_Schottland".into(), depth: 2 })
        );
        assert_eq!(source("category="), None);
    }

    #[test]
    fn alphabetical_order() {
        let list = [wanted("C", 3), wanted("A", 1), wanted("B", 2)];
        let sorted = alphabetical(&list);
        let titles: Vec<&str> = sorted.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(titles, ["A", "B", "C"]);
    }

    #[test]
    fn alphabetical_keeps_entries_beyond_the_html_limit() {
        let long: Vec<Wanted> = (0..=MAX_SHOWN).map(|i| wanted(&format!("T{i:05}"), 1)).collect();
        assert_eq!(alphabetical(&long).len(), long.len());
    }

    #[test]
    fn outputs_parse_and_round_trip() {
        assert_eq!(request("wikimode=json").wikimode, Output::Json);
        assert_eq!(request("wikimode=2").wikimode, Output::WikiList);
        assert_eq!(request("wikimode=3").wikimode, Output::WikiListByDemand);
        assert!(serde_urlencoded::from_str::<Request>("wikimode=7").is_err());
        let r = request("language=en&category=A b&wikimode=1&doit=1");
        assert_eq!(
            query(&r, &Options::new(&r)),
            "language=en&project=wikipedia&article=&category=A+b&depth=1&wikimode=1&limitnum=1&doit=1"
        );
    }

    #[test]
    fn trims_to_the_most_wanted() {
        let mut counts = HashMap::from([("A".to_string(), 1), ("B".to_string(), 3), ("C".to_string(), 2)]);
        trim_top(&mut counts, 2);
        assert_eq!(counts.len(), 2);
        assert!(counts.contains_key("B"));
        assert!(counts.contains_key("C"));
        assert!(!counts.contains_key("A"));
    }

    #[test]
    fn ranking() {
        let counts = HashMap::from([("B".to_string(), 2), ("A".to_string(), 2), ("C".to_string(), 5)]);
        let names: Vec<String> = ranked(counts).into_iter().map(|(t, _)| t).collect();
        assert_eq!(names, ["C", "A", "B"]);
    }

    #[test]
    fn wiki_lists_use_their_order() {
        let wanted = vec![wanted("B", 2), wanted("A", 1)];
        let alphabetical = alphabetical(&wanted);
        let results = Results {
            pages: 0,
            pages_truncated: false,
            results_truncated: false,
            template_links: 0,
            wanted,
            alphabetical,
        };
        let titles = |output| results.for_output(output).iter().map(|w| w.title.clone()).collect::<Vec<_>>();
        assert_eq!(titles(Output::WikiList), ["A", "B"]);
        assert_eq!(titles(Output::WikiListByDemand), ["B", "A"]);
    }

    #[test]
    fn wiki_text() {
        let list = [wanted("Foo bar", 3)];
        let again = "https://x.org/missingtopics/?a=1";
        let table = wikitext(Output::WikiTable, &list, again, "D").unwrap();
        assert!(table.starts_with("== Created D by [https://x.org/missingtopics/ the MissingTopics tool]"));
        assert!(table.contains("|-\n| 3 || [[Foo bar]]\n|}"));
        assert!(wikitext(Output::WikiList, &list, again, "D").is_some_and(|t| t.ends_with("[[Foo bar]] –\n")));
        let two = [wanted("Foo bar", 3), wanted("Baz", 1)];
        assert!(
            wikitext(Output::WikiListByDemand, &two, again, "D")
                .is_some_and(|t| t.ends_with("[[Foo bar]] –\n[[Baz]] –\n"))
        );
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
        // An old "run again" link, without `doit`, must run too.
        let again = get(dir.path(), "/missingtopics/?language=xx.evil&category=A&nosingles=1").await;
        assert_eq!(again.status.as_u16(), 400);
        let old = get(dir.path(), "/missingtopics.php?article=A").await;
        assert_eq!(old.headers["location"], "/missingtopics/?article=A");
    }
}
