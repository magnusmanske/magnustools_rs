//! `most-wanted` (`most-wanted.toolforge.org/index.php`): the red links that
//! the most articles point to, per wiki. The lists are flat files that the
//! `most-wanted` job rebuilds daily; this page reads one and batches the
//! replica queries the original ran per article.

use crate::regex;
use crate::replicas::select_in;
use crate::state::AppState;
use crate::web::{AppError, Pager, PagerLink, Params, render, thousands};
use crate::wiki::{Wiki, encode_component, encode_title, upper_first};
use anyhow::{Context, Result};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::{DateTime, Utc};
use rand::Rng;
use regex::Regex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::UNIX_EPOCH;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

const DEFAULT_PROJECT: &str = "enwiki";
const DEFAULT_NUMBER: usize = 50;
const MAX_NUMBER: usize = 1000;
const NUMBER_CHOICES: &[usize] = &[20, 50, 100, 250, 500, 1000];
/// Red links with fewer incoming links than this are not in the files.
const MIN_LINKS: u64 = 10;
/// Titles the job leaves out: they are template artifacts, not wanted articles.
const IGNORE: &[(&str, &[&str])] =
    &[("dewiki", &[r"Droga_wojewódzka_\d+", r"Europastraße_\d+", r"\bDivision\b"]), ("enwiki", &[r"\bDivision\b"])];
/// Hints that say nothing about the topic.
static BORING_HINT: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)\b(?:articles|living people)\b"));

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_project")]
    project: String,
    start: Option<String>,
    number: Option<String>,
    hints: Option<String>,
}

fn default_project() -> String {
    DEFAULT_PROJECT.to_string()
}

impl Request {
    fn project(&self) -> String {
        match self.project.trim().is_empty() {
            true => DEFAULT_PROJECT.to_string(),
            false => self.project.trim().to_string(),
        }
    }

    fn start(&self) -> usize {
        self.start.as_deref().and_then(|s| s.trim().parse().ok()).unwrap_or(1).max(1)
    }

    fn number(&self) -> usize {
        self.number.as_deref().and_then(|n| n.trim().parse().ok()).unwrap_or(DEFAULT_NUMBER).clamp(1, MAX_NUMBER)
    }

    fn hints(&self) -> bool {
        self.hints.as_deref().is_some_and(|h| !h.is_empty() && h != "0")
    }
}

/// One line of a `.links.red` file.
struct Entry {
    count: u64,
    title: String,
}

struct Wanted {
    entries: Vec<Entry>,
    updated: String,
}

/// A choice in a `<select>`, or a page navigation link.
struct Choice {
    value: String,
    label: String,
    selected: bool,
}

/// The same title with its first letter uppercased, if that page exists.
struct Alternative {
    title: String,
    url: String,
}

struct Row {
    rank: usize,
    title: String,
    url: String,
    links_url: String,
    links: String,
    google: String,
    google_wikipedia: String,
    non_template: u64,
    exists: bool,
    alternative: Option<Alternative>,
    hints: String,
}

#[derive(Template)]
#[template(path = "tools/most_wanted.html")]
struct Page {
    title: &'static str,
    project: String,
    projects: Vec<Choice>,
    numbers: Vec<Choice>,
    entries: String,
    updated: String,
    min_links: u64,
    nav: Pager,
    rows: Vec<Row>,
    show_hints: bool,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let dir = &state.config.most_wanted_dir;
    let projects = projects(dir).await;
    let number = request.number();
    let hints = request.hints();
    let start = request.start();
    let requested = request.project();
    let wiki = match Wiki::from_dbname(&requested) {
        Ok(wiki) => wiki,
        Err(e) => {
            return render(&Page {
                title: "Most wanted",
                project: requested.clone(),
                projects: project_choices(&projects, &requested),
                numbers: number_choices(number),
                entries: "0".to_string(),
                updated: String::new(),
                min_links: MIN_LINKS,
                nav: Pager::default(),
                rows: vec![],
                show_hints: hints,
                error: Some(format!("Not a wiki: “{requested}” ({e}).")),
            });
        }
    };
    let project = wiki.dbname().to_string();

    let mut page = Page {
        title: "Most wanted",
        project: project.clone(),
        projects: project_choices(&projects, &project),
        numbers: number_choices(number),
        entries: "0".to_string(),
        updated: String::new(),
        min_links: MIN_LINKS,
        nav: Pager::default(),
        rows: vec![],
        show_hints: hints,
        error: None,
    };
    if !projects.contains(&project) {
        page.error = Some(match projects.is_empty() {
            true => "There are no lists yet: the update job has not run.".to_string(),
            false => format!("Not a project with a list: “{project}”."),
        });
        return render(&page);
    }

    let Some(wanted) = load(dir, &project).await? else {
        page.error = Some(format!("The list for “{project}” has gone missing."));
        return render(&page);
    };
    let entries = wanted.entries.len();
    let slice: Vec<&Entry> = wanted.entries.iter().skip(start - 1).take(number).collect();
    let titles: Vec<String> = slice.iter().map(|e| e.title.clone()).collect();
    let mut conn =
        state.replicas.conn(&wiki, &["page", "pagelinks", "linktarget", "templatelinks", "categorylinks"]).await?;
    let existing = existing_titles(&mut conn, &titles).await?;
    let capitalized: Vec<String> = titles.iter().map(|t| upper_first(&t.to_lowercase())).collect();
    let existing_capitalized = existing_titles(&mut conn, &capitalized).await?;
    let via_templates = template_links(&mut conn, &titles).await?;
    let hints_by_title = if hints { hint_categories(&mut conn, &titles).await? } else { HashMap::new() };

    let rows = slice
        .iter()
        .zip(&capitalized)
        .enumerate()
        .map(|(index, (entry, capitalized_title))| {
            let title = &entry.title;
            let pretty = title.replace('_', " ");
            let links = via_templates.get(title).copied().unwrap_or_default();
            let alternative = match capitalized_title != title && existing_capitalized.contains(capitalized_title) {
                true => Some(Alternative {
                    title: capitalized_title.replace('_', " "),
                    url: wiki.page_url(capitalized_title),
                }),
                false => None,
            };
            Row {
                rank: start + index,
                title: pretty.clone(),
                url: wiki.page_url(title),
                links_url: wiki.index_url()
                    + &format!("?title=Special%3AWhatLinksHere&target={}&namespace=0", encode_title(title)),
                links: thousands(entry.count),
                google: format!("https://www.google.com/search?q={}", encode_component(&pretty)),
                google_wikipedia: format!(
                    "https://www.google.com/search?q={}",
                    encode_component(&format!("{pretty} site:wikipedia.org"))
                ),
                non_template: entry.count.saturating_sub(links),
                exists: existing.contains(title),
                alternative,
                hints: hints_by_title.get(title).map(|c| c.join(" | ")).unwrap_or_default(),
            }
        })
        .collect();

    page.entries = thousands(entries as u64);
    page.updated = wanted.updated;
    page.nav = navigation(&project, number, hints, start, entries);
    page.rows = rows;
    render(&page)
}

/// The projects with a list: one `<project>_p.links.red` file each.
async fn projects(dir: &Path) -> Vec<String> {
    let mut projects = vec![];
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else { return projects };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        if let Some(project) = name.to_string_lossy().strip_suffix("_p.links.red") {
            projects.push(project.to_string());
        }
    }
    projects.sort();
    projects
}

fn project_choices(projects: &[String], current: &str) -> Vec<Choice> {
    projects
        .iter()
        .map(|project| Choice { value: project.clone(), label: project.clone(), selected: project == current })
        .collect()
}

fn number_choices(number: usize) -> Vec<Choice> {
    let mut numbers = NUMBER_CHOICES.to_vec();
    if !numbers.contains(&number) {
        numbers.push(number);
        numbers.sort_unstable();
    }
    numbers.into_iter().map(|n| Choice { value: n.to_string(), label: n.to_string(), selected: n == number }).collect()
}

async fn load(dir: &Path, project: &str) -> Result<Option<Wanted>, AppError> {
    let path = dir.join(format!("{project}_p.links.red"));
    let text = match tokio::fs::read_to_string(&path).await {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::Internal(format!("cannot read {}: {e}", path.display()))),
    };
    let updated = tokio::fs::metadata(&path)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
        .map_or_else(|| "unknown".to_string(), |t| t.format("%B %d, %Y %H:%M:%S").to_string());
    Ok(Some(Wanted { entries: parse_entries(&text), updated }))
}

fn parse_entries(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|line| {
            let (count, title) = line.split_once('\t')?;
            Some(Entry { count: count.trim().parse().ok()?, title: title.trim_end().to_string() })
        })
        .filter(|entry| !entry.title.is_empty())
        .collect()
}

/// Those of `titles` (namespace 0) that exist.
async fn existing_titles(conn: &mut Conn, titles: &[String]) -> Result<HashSet<String>, AppError> {
    if titles.is_empty() {
        return Ok(HashSet::new());
    }
    let sql = "SELECT DISTINCT page_title FROM page WHERE page_namespace=0 AND page_title IN";
    let rows: Vec<String> = select_in(conn, sql, "", &[], titles).await?;
    Ok(rows.into_iter().collect())
}

/// Links that reach the articles through templates, which are not real
/// wanted articles: per article, the pages that transclude a template that
/// links to it.
async fn template_links(conn: &mut Conn, titles: &[String]) -> Result<HashMap<String, u64>, AppError> {
    if titles.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = "SELECT art.lt_title, COUNT(DISTINCT used.page_title)
        FROM linktarget art
        JOIN pagelinks pl ON pl.pl_target_id=art.lt_id AND pl.pl_from_namespace=10
        JOIN page tpl ON tpl.page_id=pl.pl_from
        JOIN linktarget tlt ON tlt.lt_namespace=10 AND tlt.lt_title=tpl.page_title
        JOIN templatelinks tl ON tl.tl_target_id=tlt.lt_id
        JOIN page used ON used.page_id=tl.tl_from AND used.page_namespace=0
        WHERE art.lt_namespace=0 AND art.lt_title IN";
    let rows: Vec<(String, u64)> = select_in(conn, sql, "GROUP BY art.lt_title", &[], titles).await?;
    Ok(rows.into_iter().collect())
}

/// Per article, the categories its linking articles share most.
async fn hint_categories(conn: &mut Conn, titles: &[String]) -> Result<HashMap<String, Vec<String>>, AppError> {
    if titles.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = "SELECT tgt.lt_title, cat.lt_title, COUNT(DISTINCT src.page_title)
        FROM linktarget tgt
        JOIN pagelinks pl ON pl.pl_target_id=tgt.lt_id AND pl.pl_from_namespace=0
        JOIN page src ON src.page_id=pl.pl_from AND src.page_namespace=0
        JOIN categorylinks cl ON cl.cl_from=src.page_id
        JOIN linktarget cat ON cat.lt_id=cl.cl_target_id AND cat.lt_namespace=14
        WHERE tgt.lt_namespace=0 AND tgt.lt_title IN";
    let rows: Vec<(String, String, u64)> =
        select_in(conn, sql, "GROUP BY tgt.lt_title, cat.lt_title", &[], titles).await?;
    let mut counts: HashMap<String, HashMap<String, u64>> = HashMap::new();
    for (article, category, count) in rows {
        counts.entry(article).or_default().insert(category, count);
    }
    let mut hints = HashMap::new();
    for (article, categories) in counts {
        // Some articles have no linking category; the original's `max()`
        // on that empty list was a fatal error.
        let Some(top) = categories.values().max().copied() else { continue };
        let mut names: Vec<String> = categories
            .into_iter()
            .filter(|(_, count)| *count == top)
            .map(|(category, _)| category.replace('_', " "))
            .filter(|category| !boring_hint(category))
            .collect();
        names.sort();
        hints.insert(article, names);
    }
    Ok(hints)
}

fn boring_hint(category: &str) -> bool {
    category == "Mann" || category == "Frau" || BORING_HINT.is_match(category)
}

/// Start, Previous, Next, End and Random, as the original had them.
fn navigation(project: &str, number: usize, hints: bool, start: usize, entries: usize) -> Pager {
    let at = |start: usize| url(project, number, hints, start);
    let mut nav = vec![];
    if start > 1 {
        nav.push(PagerLink::new("Start", at(1)));
        nav.push(PagerLink::new(format!("Previous {number}"), at(start.saturating_sub(number).max(1))));
    } else {
        nav.push(PagerLink::disabled("Start"));
        nav.push(PagerLink::disabled(format!("Previous {number}")));
    }
    let end = entries.saturating_sub(number).saturating_add(1);
    if start < end {
        nav.push(PagerLink::new(format!("Next {number}"), at(start + number)));
        nav.push(PagerLink::new("End", at(end)));
    } else {
        nav.push(PagerLink::disabled(format!("Next {number}")));
        nav.push(PagerLink::disabled("End"));
    }
    let random = match entries {
        0 => 1,
        entries => rand::rng().random_range(0..entries) / number * number + 1,
    };
    nav.push(PagerLink::new("Random", at(random)));
    Pager::new(nav)
}

fn url(project: &str, number: usize, hints: bool, start: usize) -> String {
    format!(
        "/most_wanted/?project={}&number={number}&hints={}&start={start}",
        encode_component(project),
        u8::from(hints)
    )
}

/// Rebuilds `<project>_p.links.red`: the red links with at least
/// [`MIN_LINKS`] incoming links from articles, most links first.
pub(crate) async fn update(state: &AppState, wiki: &str) -> Result<()> {
    let wiki = Wiki::from_dbname(wiki).map_err(|e| anyhow::anyhow!("{e}"))?;
    let project = wiki.dbname();
    let mut conn = state.replicas.conn(&wiki, &["linktarget", "pagelinks", "page"]).await?;
    let max_id: Option<u64> = conn.exec_first("SELECT MAX(lt_id) FROM linktarget", ()).await?;
    let max_id = max_id.unwrap_or(0);
    let ignore: Vec<Regex> = IGNORE
        .iter()
        .filter(|(name, _)| *name == project)
        .flat_map(|(_, patterns)| patterns.iter().filter_map(|p| Regex::new(p).ok()))
        .collect();

    let mut rows: Vec<(u64, String)> = vec![];
    let mut from = 1;
    while from <= max_id {
        let to = from.saturating_add(CHUNK - 1);
        let found: Vec<(u64, String)> = conn
            .exec(
                "SELECT COUNT(*) AS cnt, lt_title FROM linktarget
                 JOIN pagelinks ON pl_target_id=lt_id AND pl_from_namespace=0
                 WHERE lt_namespace=0 AND lt_id BETWEEN ? AND ?
                 AND NOT EXISTS (SELECT 1 FROM page WHERE page_namespace=0 AND page_title=lt_title)
                 GROUP BY lt_id HAVING cnt >= ?",
                (from, to, MIN_LINKS),
            )
            .await
            .with_context(|| format!("{project}: linktarget {from}–{to}"))?;
        rows.extend(found.into_iter().filter(|(_, title)| !ignore.iter().any(|re| re.is_match(title))));
        tracing::info!("{project}: scanned linktarget up to {to}");
        from = to.saturating_add(1);
    }
    rows.sort_by(|(left_count, left_title), (right_count, right_title)| {
        right_count.cmp(left_count).then_with(|| left_title.cmp(right_title))
    });

    // Write a new file and rename it, so the page never sees a partial list.
    let dir = &state.config.most_wanted_dir;
    tokio::fs::create_dir_all(dir).await.with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join(format!("{project}_p.links.red"));
    let temporary = dir.join(format!("{project}_p.links.red.tmp"));
    let mut text = String::new();
    for (count, title) in &rows {
        writeln!(text, "{count}\t{title}")?;
    }
    tokio::fs::write(&temporary, text.as_bytes())
        .await
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    tokio::fs::rename(&temporary, &path).await.with_context(|| format!("cannot replace {}", path.display()))?;
    println!("{} entries written to {}", rows.len(), path.display());
    Ok(())
}

/// `linktarget` IDs per query; keeps each scan short.
const CHUNK: u64 = 1_000_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_count_tab_title() {
        let entries = parse_entries("42\tFoo_bar\n7\tBaz\n\nbroken\n3\t\n");
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].count, entries[0].title.as_str()), (42, "Foo_bar"));
        assert_eq!((entries[1].count, entries[1].title.as_str()), (7, "Baz"));
    }

    #[test]
    fn parameters_have_the_old_defaults_and_limits() {
        let request = |query: &str| serde_urlencoded::from_str::<Request>(query).unwrap();
        let plain = request("");
        assert_eq!((plain.project(), 1, 50, false), ("enwiki".to_string(), 1, 50, false));
        let set = request("project=dewiki_p&start=101&number=250&hints=1");
        assert_eq!((set.project(), set.start(), set.number(), set.hints()), ("dewiki_p".to_string(), 101, 250, true));
        assert_eq!(request("number=5000&start=0&hints=0").number(), MAX_NUMBER);
        assert_eq!(request("start=0").start(), 1);
        assert!(!request("hints=0").hints());
    }

    #[test]
    fn project_names_come_from_database_names() {
        assert_eq!(Wiki::from_dbname("enwiki_p").unwrap().dbname(), "enwiki");
        assert_eq!(Wiki::from_dbname("dewikisource").unwrap().dbname(), "dewikisource");
        assert!(Wiki::from_dbname("not a wiki").is_err());
    }

    #[test]
    fn navigation_mirrors_the_original() {
        let labels = |start| {
            navigation("enwiki", 50, false, start, 1000).links.iter().map(|l| l.label.clone()).collect::<Vec<_>>()
        };
        assert_eq!(labels(1), ["Start", "Previous 50", "Next 50", "End", "Random"]);
        assert_eq!(labels(501), ["Start", "Previous 50", "Next 50", "End", "Random"]);
        let first = navigation("enwiki", 50, false, 1, 1000);
        assert!(first.links[0].is_disabled() && first.links[1].is_disabled());
        assert!(!first.links[2].is_disabled());
        let urls: Vec<&str> = first.links.iter().map(|l| l.url.as_deref().unwrap_or_default()).collect();
        assert_eq!(urls[2], "/most_wanted/?project=enwiki&number=50&hints=0&start=51");
        let last = navigation("enwiki", 50, false, 951, 1000);
        assert!(last.links[2].is_disabled() && last.links[3].is_disabled());
    }

    #[test]
    fn hints_say_nothing_about_nothing() {
        assert!(boring_hint("Articles"));
        assert!(boring_hint("Living people"));
        assert!(boring_hint("Mann"));
        assert!(boring_hint("Frau"));
        assert!(!boring_hint("Birds of Europe"));
    }

    fn sample_page() -> Page {
        Page {
            title: "Most wanted",
            project: "enwiki".into(),
            projects: vec![Choice { value: "enwiki".into(), label: "enwiki".into(), selected: true }],
            numbers: number_choices(50),
            entries: "1,000".to_string(),
            updated: "November 11, 2026 10:00:00".into(),
            min_links: MIN_LINKS,
            nav: navigation("enwiki", 50, false, 1, 1000),
            rows: vec![Row {
                rank: 1,
                title: "Foo bar".into(),
                url: "https://en.wikipedia.org/wiki/Foo_bar".into(),
                links_url: "https://en.wikipedia.org/w/index.php".into(),
                links: "42".into(),
                google: "https://www.google.com/".into(),
                google_wikipedia: "https://www.google.com/".into(),
                non_template: 12,
                exists: false,
                alternative: None,
                hints: String::new(),
            }],
            show_hints: false,
            error: None,
        }
    }

    #[test]
    fn the_page_shows_the_list() {
        let html = sample_page().render().unwrap();
        assert!(html.contains("<h1>Most wanted</h1>"));
        assert!(html.contains("#1"));
        assert!(html.contains("redlink"));
        assert!(html.contains("Foo bar"));
        assert!(html.contains("1,000 potential articles on enwiki"));
        assert_eq!(html.matches("<nav class=\"pagination\" aria-label=\"Pagination\">").count(), 2);
        assert!(html.contains("<span class=\"muted\" aria-disabled=\"true\">Start</span>"));
    }

    #[test]
    fn an_error_replaces_the_list() {
        let mut page = sample_page();
        page.error = Some("Not a project with a list".into());
        let html = page.render().unwrap();
        assert!(html.contains("<p class=\"error\">Not a project with a list</p>"));
        assert!(!html.contains("Foo bar"));
    }
}
