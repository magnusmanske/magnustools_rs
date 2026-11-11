//! `map2wp.php`: candidate English Wikipedia articles for Wikisource DNB
//! biographies that have no Wikipedia link yet.

use super::dnb;
use crate::mediawiki;
use crate::regex;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use regex::Regex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};
use tokio::task::JoinSet;

const TITLE: &str = "Map DNB to Wikipedia";
const DEFAULT_SET: &str = "dnb00";
/// Current DNB biographies on Wikisource live below this prefix.
const DNB_PREFIX: &str = "Dictionary_of_National_Biography,_1885-1900/";
/// Pages checked per request; a longer letter prefix narrows the list.
const MAX_PAGES: usize = 100;
/// Concurrent Wikisource and Wikipedia requests.
const CONCURRENCY: usize = 4;
const SEARCH_LIMIT: usize = 50;
const INTRO_CHARS: usize = 400;

static DNB_SUFFIX: LazyLock<Regex> = LazyLock::new(|| regex(r"\s*\(DNB\d+\)$"));
static FLOURISHED: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)\bfl\."));
static DIED: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)\bd\."));
static DIGITS: LazyLock<Regex> = LazyLock::new(|| regex(r"[0-9()\-]"));
static FOUR_DIGITS: LazyLock<Regex> = LazyLock::new(|| regex(r"\b\d{4}\b"));
static THREE_OR_FOUR_DIGITS: LazyLock<Regex> = LazyLock::new(|| regex(r"\b\d{3,4}\b"));
static BIRTH_DEATH: LazyLock<Regex> = LazyLock::new(|| regex(r"^(\d+) (births|deaths)$"));
static VOLUME: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)\|\s*volume\s*=\s*([^|\n}]+)"));

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    set: String,
    letter: String,
}

impl Default for Request {
    fn default() -> Self {
        Self { set: DEFAULT_SET.to_string(), letter: String::new() }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    Dnb00,
    Ce13,
    Appletons,
}

impl Set {
    fn from_param(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "ce13" => Self::Ce13,
            "appletons" => Self::Appletons,
            _ => Self::Dnb00,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Dnb00 => "dnb00",
            Self::Ce13 => "ce13",
            Self::Appletons => "appletons",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Dnb00 => "DNB00",
            Self::Ce13 => "Catholic Encyclopedia (1913)",
            Self::Appletons => "Appletons' Cyclopædia",
        }
    }

    fn category(self) -> &'static str {
        match self {
            Self::Dnb00 => "DNB No WP",
            Self::Ce13 => "CE No WP",
            Self::Appletons => "Appletons No WP",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Dnb00 => DNB_PREFIX,
            Self::Ce13 => "Catholic Encyclopedia (1913)/",
            Self::Appletons => "Appletons' Cyclopædia of American Biography/",
        }
    }

    fn ignore(self, word: &str) -> bool {
        let word = word.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        let common = matches!(word.as_str(), "de" | "of");
        match self {
            Self::Dnb00 => common,
            _ => common || matches!(word.as_str(), "ven" | "venerable" | "bl" | "blessed" | "st" | "saint" | "pope"),
        }
    }

    fn citation(self, title: &str, volume: &str) -> String {
        match self {
            Self::Dnb00 if volume.is_empty() => format!("{{{{cite DNB|wstitle={title}}}}}"),
            Self::Dnb00 => format!("{{{{cite DNB|wstitle={title}|volume={volume}}}}}"),
            Self::Ce13 => format!("{{{{1913CE|{title}}}}}"),
            Self::Appletons => format!("{{{{Cite Appletons'|{title}}}}}"),
        }
    }
}

struct PageRef {
    full: String,
    search: String,
}

struct Candidate {
    title: String,
    url: String,
    categories: Vec<String>,
    matches: usize,
}

impl Candidate {
    fn categories_text(&self) -> String {
        self.categories.join(", ")
    }
}

struct Row {
    ws_title: String,
    ws_url: String,
    years: String,
    intro: String,
    candidates: Vec<Candidate>,
    more: bool,
    citation: String,
}

#[derive(Template)]
#[template(path = "tools/map2wp.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    set_code: &'static str,
    set_label: &'static str,
    letter: String,
    total: usize,
    capped: bool,
    failed: usize,
    rows: Vec<Row>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let set = Set::from_param(&request.set);
    let letter = request.letter.trim().to_uppercase();
    let wikisource = Wiki::new("en", "wikisource")?;
    if letter.is_empty() {
        return render(&Page {
            title: TITLE,
            request: &request,
            set_code: set.code(),
            set_label: set.label(),
            letter,
            total: 0,
            capped: false,
            failed: 0,
            rows: Vec::new(),
        });
    }
    let wikipedia = Wiki::new("en", "wikipedia")?;
    let pages = pages(&state, &wikisource, set, &letter).await?;
    let total = pages.len();
    let capped = total > MAX_PAGES;
    let pages = pages.into_iter().take(MAX_PAGES).collect();
    let (mut rows, failed) = check_pages(state, wikisource, wikipedia, set, pages).await;
    rows.sort_by(|a, b| a.ws_title.cmp(&b.ws_title));
    render(&Page {
        title: TITLE,
        request: &request,
        set_code: set.code(),
        set_label: set.label(),
        letter,
        total,
        capped,
        failed,
        rows,
    })
}

async fn pages(state: &AppState, wiki: &Wiki, set: Set, letter: &str) -> Result<Vec<PageRef>, AppError> {
    let mut titles = state.replicas.pages_in_tree(wiki, set.category(), 1, 0).await?;
    if set == Set::Dnb00 {
        let see: HashSet<String> = state.replicas.pages_in_tree(wiki, "DNB See", 0, 0).await?.into_iter().collect();
        titles.retain(|title| !see.contains(title));
    }
    let mut pages: Vec<PageRef> = titles
        .into_iter()
        .filter_map(|full| {
            let search = full.strip_prefix(set.prefix()).unwrap_or(&full);
            let search = dnb::display(search);
            search.to_uppercase().starts_with(letter).then_some(PageRef { full, search })
        })
        .collect();
    pages.sort_by(|a, b| a.search.cmp(&b.search));
    Ok(pages)
}

async fn check_pages(
    state: Arc<AppState>,
    wikisource: Wiki,
    wikipedia: Wiki,
    set: Set,
    pages: Vec<PageRef>,
) -> (Vec<Row>, usize) {
    let mut rows = Vec::with_capacity(pages.len());
    let mut failed = 0;
    let mut tasks = JoinSet::new();
    let mut pages = pages.into_iter();
    let mut in_flight = 0;
    loop {
        while in_flight < CONCURRENCY {
            let Some(page) = pages.next() else {
                break;
            };
            let state = state.clone();
            let wikisource = wikisource.clone();
            let wikipedia = wikipedia.clone();
            tasks.spawn(async move { check_page(&state, &wikisource, &wikipedia, set, page).await });
            in_flight += 1;
        }
        if in_flight == 0 {
            break;
        }
        match tasks.join_next().await {
            Some(Ok(Ok(row))) => rows.push(row),
            Some(Ok(Err(error))) => {
                tracing::warn!("map2wp: {error}");
                failed += 1;
            }
            Some(Err(error)) => {
                tracing::warn!("map2wp task: {error}");
                failed += 1;
            }
            None => break,
        }
        in_flight -= 1;
    }
    (rows, failed)
}

async fn check_page(
    state: &AppState,
    wikisource: &Wiki,
    wikipedia: &Wiki,
    set: Set,
    page: PageRef,
) -> Result<Row, String> {
    let parse = mediawiki::api_patiently(
        &state.http,
        wikisource,
        &[("action", "parse"), ("page", &page.full), ("prop", "text|wikitext"), ("redirects", "1")],
    )
    .await
    .map_err(|error| error.to_string())?;
    let html = parse["parse"]["text"].as_str().unwrap_or_default();
    let wikitext = parse["parse"]["wikitext"].as_str().unwrap_or_default();
    let volume = VOLUME
        .captures(wikitext)
        .and_then(|captures| captures.get(1))
        .map_or_else(String::new, |value| value.as_str().trim().to_string());
    let intro = first_paragraph(html).unwrap_or_default();
    let citation_title = match set {
        Set::Dnb00 => DNB_SUFFIX.replace(&page.search, "").trim().to_string(),
        _ => page.search.clone(),
    };
    let years = years(&citation_title, &intro);
    let search = search_title(&citation_title, set);
    let ws_title = citation_title.clone();
    let (mut candidates, more) =
        find_candidates(&state.http, wikipedia, &search, &years).await.map_err(|error| error.to_string())?;
    candidates.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(Row {
        ws_title,
        ws_url: wikisource.page_url(&page.full),
        years: if years.is_empty() { String::new() } else { format!(" ({})", years.join("-")) },
        intro,
        candidates,
        more,
        citation: set.citation(&citation_title, &volume),
    })
}

async fn find_candidates(
    http: &reqwest::Client,
    wikipedia: &Wiki,
    search: &str,
    years: &[String],
) -> Result<(Vec<Candidate>, bool), AppError> {
    let parts: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
    if parts.len() < 2 {
        return Ok((Vec::new(), false));
    }
    let limit = SEARCH_LIMIT.to_string();
    let json = mediawiki::api_patiently(
        http,
        wikipedia,
        &[("action", "query"), ("list", "search"), ("srsearch", search), ("srlimit", &limit), ("srnamespace", "0")],
    )
    .await?;
    let more = json.get("continue").is_some();
    let titles: Vec<String> = json["query"]["search"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|hit| hit["title"].as_str())
        .filter(|title| {
            let title = title.to_lowercase();
            parts.iter().filter(|part| title.contains(part.as_str())).count() >= 2
        })
        .map(str::to_string)
        .collect();
    if titles.is_empty() {
        return Ok((Vec::new(), false));
    }

    let titles = titles.join("|");
    let categories = mediawiki::api_patiently(
        http,
        wikipedia,
        &[("action", "query"), ("prop", "categories"), ("titles", &titles), ("cllimit", "500")],
    )
    .await?;
    let mut by_page: HashMap<String, Vec<String>> = HashMap::new();
    for page in categories["query"]["pages"].as_array().into_iter().flatten() {
        let Some(title) = page["title"].as_str() else {
            continue;
        };
        let categories = page["categories"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|category| category["title"].as_str())
            .map(|category| category.strip_prefix("Category:").unwrap_or(category).to_string())
            .collect();
        by_page.insert(title.to_string(), categories);
    }

    let mut candidates = Vec::new();
    for title in titles.split('|') {
        let categories = by_page.get(title).cloned().unwrap_or_default();
        if categories.iter().any(|category| category == "Living people") {
            continue;
        }
        let mut matches = 0;
        let mut birth_death = Vec::new();
        for category in &categories {
            let Some(captures) = BIRTH_DEATH.captures(category) else {
                continue;
            };
            birth_death.push(category.clone());
            if years.contains(&captures[1].to_string()) {
                matches += 1;
            }
        }
        candidates.push(Candidate {
            title: title.to_string(),
            url: wikipedia.page_url(title),
            categories: birth_death,
            matches,
        });
    }
    let more = more && !candidates.is_empty();
    Ok((candidates, more))
}

/// The first substantial paragraph of a parsed Wikisource page.
fn first_paragraph(html: &str) -> Option<String> {
    let html = remove_tag_blocks(html, "style");
    let html = remove_tag_blocks(&html, "script");
    let start = html.find("header_notes").or_else(|| html.find("header&#95;notes")).unwrap_or(0);
    let mut rest = &html[start..];
    while let Some(open) = rest.find("<p") {
        let after = &rest[open..];
        let Some(text_start) = after.find('>') else {
            break;
        };
        let after = &after[text_start + 1..];
        let Some(text_end) = after.find("</p>") else {
            break;
        };
        let text = collapse_whitespace(&mediawiki::html_to_text(&after[..text_end]));
        if text.len() >= 50 && !text.to_lowercase().starts_with("errata") {
            return Some(truncate_words(&text, INTRO_CHARS));
        }
        rest = &after[text_end + 4..];
    }
    None
}

fn remove_tag_blocks(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(&open) {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find(&close) else {
            return out;
        };
        rest = &rest[start + end + close.len()..];
    }
    out.push_str(rest);
    out
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_words(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for word in text.split_whitespace() {
        if out.len() + word.len() + 1 > max {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() { text.chars().take(max).collect() } else { format!("{out}…") }
}

fn search_title(title: &str, set: Set) -> String {
    let mut title = title.to_string();
    if set == Set::Dnb00 {
        title = DNB_SUFFIX.replace(&title, "").to_string();
        title = title.replace('?', "");
        title = FLOURISHED.replace_all(&title, " ").to_string();
        title = DIED.replace_all(&title, " ").to_string();
    }
    title = DIGITS.replace_all(&title, "").to_string();
    title = title.replace(',', " ");
    title.split_whitespace().filter(|word| !set.ignore(word)).collect::<Vec<_>>().join(" ")
}

fn years(title: &str, intro: &str) -> Vec<String> {
    let mut years: Vec<String> = FOUR_DIGITS.find_iter(title).map(|year| year.as_str().to_string()).collect();
    if years.is_empty() {
        years = THREE_OR_FOUR_DIGITS.find_iter(intro).map(|year| year.as_str().to_string()).collect();
        years.truncate(2);
        normalize_years(&mut years);
    }
    years.sort();
    years.dedup();
    years
}

/// Expands a short second year (`02` after `1800`) and rolls it over if needed.
fn normalize_years(years: &mut [String]) {
    if years.len() != 2 {
        return;
    }
    let (Ok(first), Ok(second)) = (years[0].parse::<i32>(), years[1].parse::<i32>()) else {
        return;
    };
    if first <= second {
        return;
    }
    if years[1].len() < years[0].len() {
        let prefix = &years[0][..years[0].len() - years[1].len()];
        years[1] = format!("{prefix}{}", years[1]);
    }
    if first + 20 > years[1].parse().unwrap_or(0) {
        years[1] = (years[1].parse::<i32>().unwrap_or(0) + 100).to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[tokio::test]
    async fn form_renders() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/map2wp/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("<h1>Map DNB to Wikipedia</h1>"));
        assert!(page.body.contains("name=\"letter\""));
    }

    #[tokio::test]
    async fn old_address_redirects() {
        let dir = tempfile::tempdir().unwrap();
        let old = get(dir.path(), "/map2wp.php?set=ce13&letter=A").await;
        assert_eq!(old.status.as_u16(), 308);
        assert_eq!(old.headers["location"], "/map2wp/?set=ce13&letter=A");
    }

    #[test]
    fn search_titles_are_cleaned() {
        assert_eq!(search_title("Abbot, George (1562-1633)", Set::Dnb00), "Abbot George");
        assert_eq!(search_title("Abbot, George (1562-1633) (DNB00)", Set::Dnb00), "Abbot George");
        assert_eq!(search_title("Abbot, George (1562-1633)", Set::Ce13), "Abbot George");
    }

    #[test]
    fn years_from_title_or_intro() {
        assert_eq!(years("Abbot, George (1562-1633)", ""), ["1562", "1633"]);
        assert_eq!(years("Someone", "Born 1810, died 1872."), ["1810", "1872"]);
        assert_eq!(years("Someone", "Born 1810, died 872."), ["1810", "1872"]);
    }

    #[test]
    fn citations() {
        assert_eq!(Set::Dnb00.citation("Abbot, George", "01"), "{{cite DNB|wstitle=Abbot, George|volume=01}}");
        assert_eq!(Set::Ce13.citation("Abbot, George", ""), "{{1913CE|Abbot, George}}");
        assert_eq!(Set::Appletons.citation("Abbot, George", ""), "{{Cite Appletons'|Abbot, George}}");
    }

    #[test]
    fn paragraphs_skip_header_noise() {
        let html = "<div class=\"header_notes\"><p>1904 Errata appended.</p></div><p><style>x</style>This is the actual article text, long enough to be used as the introduction.</p>";
        assert_eq!(
            first_paragraph(html).as_deref(),
            Some("This is the actual article text, long enough to be used as the introduction.")
        );
    }

    #[test]
    fn page_display_keeps_prefix() {
        assert_eq!(dnb::display("Abbot,_George_(DNB00)"), "Abbot, George (DNB00)");
    }
}
