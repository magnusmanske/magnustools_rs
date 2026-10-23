//! `redirector.php`: redirects a wiki may want. For the articles in a
//! category tree, the titles of their counterparts in big Wikipedias (and
//! the redirects to those) are candidates; a candidate that local articles
//! link to, but that does not exist, is a redirect worth creating.
//! Articles without missing candidates are left out.

use crate::mediawiki;
use crate::replicas::{db_title, display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use wikimisc::mysql_async::Value;

/// The Wikipedias whose titles are candidates.
const LANGUAGES: &[&str] = &["de", "en", "es", "fr", "it", "ja", "nl", "pl", "pt", "sv"];
const MAX_DEPTH: u32 = 10;
const MAX_PAGES: usize = 500;
/// Titles per API request.
const API_BATCH: usize = 50;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    category: String,
    depth: Option<String>,
    hidezero: Option<String>,
    doit: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self { language: "de".to_string(), category: String::new(), depth: None, hidezero: None, doit: None }
    }
}

struct Candidate {
    title: String,
    url: String,
    /// Links from articles to this title.
    links: u64,
    /// Wikipedias with an article or redirect of this title.
    languages: String,
    /// Wikitext of the redirect.
    redirect: String,
}

struct Row {
    title: String,
    url: String,
    missing: Vec<Candidate>,
    existing: Vec<Candidate>,
}

struct Results {
    pages: usize,
    truncated: bool,
    rows: Vec<Row>,
    /// Where the "create" forms post to.
    index_url: String,
}

#[derive(Template)]
#[template(path = "tools/redirector.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    depth: u32,
    hide_unwanted: bool,
    languages: String,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let depth = request.depth.as_deref().and_then(|d| d.trim().parse().ok()).unwrap_or(0).min(MAX_DEPTH);
    // Without a submitted form, the box starts ticked.
    let hide_unwanted = request.doit.is_none() || request.hidezero.is_some();
    let results = match request.category.trim().is_empty() {
        true => None,
        false => {
            let wiki = Wiki::new(&request.language, "wikipedia")?;
            Some(results(&state, &wiki, &request.category, depth, hide_unwanted).await?)
        }
    };
    render(&Page {
        title: "Redirector",
        request: &request,
        depth,
        hide_unwanted,
        languages: LANGUAGES.join(", "),
        results,
    })
}

async fn results(
    state: &AppState,
    wiki: &Wiki,
    category: &str,
    depth: u32,
    hide_unwanted: bool,
) -> Result<Results, AppError> {
    // One more than needed, to know whether there are more.
    let mut pages = state.replicas.some_pages_in_tree(wiki, category, depth, 0, Some(MAX_PAGES + 1)).await?;
    let truncated = pages.len() > MAX_PAGES;
    pages.truncate(MAX_PAGES);
    pages.sort();

    // Each page's counterparts in the big Wikipedias.
    let mut conn = state.replicas.conn(wiki, &["page", "langlinks"]).await?;
    let languages: Vec<&str> = LANGUAGES.iter().copied().filter(|l| wiki.dbname() != format!("{l}wiki")).collect();
    let sql = format!(
        "SELECT page_title, ll_lang, ll_title FROM page JOIN langlinks ON ll_from=page_id
         WHERE page_namespace=0 AND ll_lang IN ({}) AND page_title IN",
        vec!["?"; languages.len()].join(",")
    );
    let params: Vec<Value> = languages.iter().map(|l| Value::from(*l)).collect();
    let links: Vec<(String, String, String)> = select_in(&mut conn, &sql, "", &params, &pages).await?;

    // Candidates per page: those titles, and the redirects to them; with the languages that have them.
    let mut by_language: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (_, language, title) in &links {
        by_language.entry(language.clone()).or_default().insert(title.clone());
    }
    let mut redirects = Redirects::new();
    for (language, titles) in &by_language {
        let other = Wiki::new(language, "wikipedia")?;
        let titles: Vec<&String> = titles.iter().collect();
        for (target, redirect) in redirects_to(&state.http, &other, &titles).await? {
            redirects.entry((language.clone(), target)).or_default().push(redirect);
        }
    }
    let candidates = candidates(&links, &redirects);

    // What the wiki has of them, and how often its articles link to them.
    let all: Vec<String> =
        candidates.values().flat_map(|c| c.keys().cloned()).collect::<BTreeSet<_>>().into_iter().collect();
    let existing = state.replicas.existing(wiki, 0, &all).await?;
    let counts = state.replicas.link_counts(wiki, &all, true).await?;

    let rows = pages
        .iter()
        .filter_map(|page| {
            let mut found: Vec<(bool, Candidate)> = candidates
                .get(page)
                .into_iter()
                .flatten()
                .map(|(title, languages)| {
                    let candidate = Candidate {
                        title: display_title(title),
                        url: wiki.page_url(title),
                        links: counts.get(title).copied().unwrap_or_default(),
                        languages: languages.iter().cloned().collect::<Vec<_>>().join(", "),
                        redirect: format!("#REDIRECT [[{}]]", display_title(page)),
                    };
                    (existing.contains(title), candidate)
                })
                .filter(|(_, c)| !hide_unwanted || c.links > 0)
                .collect();
            found.sort_by(|(_, a), (_, b)| b.links.cmp(&a.links).then_with(|| a.title.cmp(&b.title)));
            let (existing, missing): (Vec<_>, Vec<_>) = found.into_iter().partition(|(exists, _)| *exists);
            if missing.is_empty() {
                return None;
            }
            Some(Row {
                title: display_title(page),
                url: wiki.page_url(page),
                missing: missing.into_iter().map(|(_, c)| c).collect(),
                existing: existing.into_iter().map(|(_, c)| c).collect(),
            })
        })
        .collect();
    Ok(Results { pages: pages.len(), truncated, rows, index_url: wiki.index_url() })
}

/// The redirects to a title, per `(language, title)`.
type Redirects = HashMap<(String, String), Vec<String>>;

/// Per page: candidate titles (database form) from its language links `(page, language, title)`,
/// with the languages that have them.
fn candidates(
    links: &[(String, String, String)],
    redirects: &Redirects,
) -> BTreeMap<String, BTreeMap<String, BTreeSet<String>>> {
    let mut candidates: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for (page, language, title) in links {
        let titles = redirects.get(&(language.clone(), title.clone())).into_iter().flatten().chain([title]);
        for title in titles {
            let title = db_title(title);
            if title != *page {
                candidates.entry(page.clone()).or_default().entry(title).or_default().insert(language.clone());
            }
        }
    }
    candidates
}

/// (target, redirect) for the redirects to `titles`, from articles.
async fn redirects_to(
    http: &reqwest::Client,
    wiki: &Wiki,
    titles: &[&String],
) -> anyhow::Result<Vec<(String, String)>> {
    let mut pairs = vec![];
    for batch in titles.chunks(API_BATCH) {
        let titles = batch.iter().map(|t| t.as_str()).collect::<Vec<_>>().join("|");
        let mut cont: Option<String> = None;
        loop {
            let mut params = vec![
                ("action", "query"),
                ("titles", titles.as_str()),
                ("prop", "redirects"),
                ("rdnamespace", "0"),
                ("rdlimit", "max"),
            ];
            if let Some(c) = &cont {
                params.push(("rdcontinue", c.as_str()));
            }
            let json = mediawiki::api(http, wiki, &params).await?;
            for page in json["query"]["pages"].as_array().into_iter().flatten() {
                let Some(target) = page["title"].as_str() else { continue };
                for redirect in page["redirects"].as_array().into_iter().flatten() {
                    if let Some(redirect) = redirect["title"].as_str() {
                        pairs.push((target.to_string(), redirect.to_string()));
                    }
                }
            }
            cont = json["continue"]["rdcontinue"].as_str().map(str::to_string);
            if cont.is_none() {
                break;
            }
        }
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_from_titles_and_redirects() {
        let s = |v: &str| v.to_string();
        let links = vec![
            (s("Italienisches_Konzert"), s("en"), s("Italian Concerto")),
            (s("Italienisches_Konzert"), s("pt"), s("Concerto italiano")),
            (s("Italienisches_Konzert"), s("fr"), s("Italienisches Konzert")),
        ];
        let redirects = Redirects::from([((s("pt"), s("Concerto italiano")), vec![s("Concerto Italiano")])]);
        let found = candidates(&links, &redirects);
        let page = &found["Italienisches_Konzert"];
        assert_eq!(page.keys().collect::<Vec<_>>(), ["Concerto_Italiano", "Concerto_italiano", "Italian_Concerto"]);
        assert_eq!(page["Italian_Concerto"], BTreeSet::from([s("en")]));
    }
}
