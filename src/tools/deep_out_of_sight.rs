//! `deep_out_of_sight.php`: pages in a category tree whose latest revision
//! has not been sighted yet.

use super::sighting;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use wikimisc::escape;

const TITLE: &str = "Deep out-of-sight";
const DEFAULT_DEPTH: u32 = 1;
/// Revisions the API returns for an extended note, as the PHP tool did.
const REVISIONS: usize = 20;
/// API calls for extended notes; a huge category would otherwise take minutes.
const MAX_NOTES: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    project: String,
    category: String,
    depth: Option<String>,
    mode: Option<String>,
    extended: Option<String>,
    simple: Option<String>,
    sortby: String,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            language: sighting::default_language(),
            project: sighting::default_project(),
            category: String::new(),
            depth: None,
            mode: None,
            extended: None,
            simple: None,
            sortby: "category".to_string(),
        }
    }
}

#[derive(Clone)]
struct Note {
    diff: i64,
    revisions: usize,
    authors: Vec<String>,
    too_old: bool,
}

impl Note {
    fn text(&self) -> String {
        if self.too_old {
            return format!("more than {REVISIONS} revisions old");
        }
        let diff = match self.diff {
            0 => "±0".to_string(),
            diff if diff > 0 => format!("+{diff}"),
            diff => diff.to_string(),
        };
        let revisions = match self.revisions {
            1 => "1 revision".to_string(),
            revisions => format!("{revisions} revisions"),
        };
        let authors = match self.authors.is_empty() {
            true => String::new(),
            false => format!("; edited by {}", self.authors.join(", ")),
        };
        format!("{diff} chars difference; {revisions} since last sight{authors}")
    }
}

#[derive(Clone)]
struct PageRow {
    title: String,
    url: String,
    diff: i64,
    note: Option<Note>,
}

impl PageRow {
    fn diff_text(&self) -> String {
        match self.diff {
            0 => "±0".to_string(),
            diff if diff > 0 => format!("+{diff}"),
            diff => diff.to_string(),
        }
    }

    fn diff_class(&self) -> &'static str {
        match self.diff {
            diff if diff > 0 => "diff-plus",
            diff if diff < 0 => "diff-minus",
            _ => "",
        }
    }
}

struct Category {
    title: String,
    url: String,
    pages: Vec<PageRow>,
}

struct Results {
    groups: Vec<Category>,
    /// Set for `sortby=size`, so the template can show one ordered list.
    sorted: Option<Vec<PageRow>>,
    found: usize,
    truncated: bool,
}

#[derive(Template)]
#[template(path = "tools/deep_out_of_sight.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    depth: u32,
    rss: bool,
    extended: bool,
    simple: bool,
    size_sorted: bool,
    notes_capped: bool,
    notes_limit: usize,
    results: Option<Results>,
}

struct Hit {
    category: String,
    title: String,
    diff: i64,
    stable: u64,
    note: Option<Note>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let wiki = Wiki::new(&request.language, &request.project)?;
    let depth = sighting::depth(request.depth.as_deref(), DEFAULT_DEPTH);
    let rss_output = sighting::is_rss(request.mode.as_deref());
    let extended = request.extended.is_some();
    let simple = request.simple.is_some();
    let size_sorted = request.sortby.eq_ignore_ascii_case("size");
    let category = request.category.trim();
    if category.is_empty() {
        return Ok(render(&Page {
            title: TITLE,
            request: &request,
            depth,
            rss: rss_output,
            extended,
            simple,
            size_sorted,
            notes_capped: false,
            notes_limit: MAX_NOTES,
            results: None,
        })?
        .into_response());
    }
    let (results, notes_capped) = results(&state, &wiki, category, depth, extended, size_sorted).await?;
    if rss_output {
        return Ok(sighting::rss_response(rss(&request, category, &results)));
    }
    Ok(render(&Page {
        title: TITLE,
        request: &request,
        depth,
        rss: rss_output,
        extended,
        simple,
        size_sorted,
        notes_capped,
        notes_limit: MAX_NOTES,
        results: Some(results),
    })?
    .into_response())
}

async fn results(
    state: &AppState,
    wiki: &Wiki,
    category: &str,
    depth: u32,
    extended: bool,
    size_sorted: bool,
) -> Result<(Results, bool), AppError> {
    let tree = state.replicas.category_tree(wiki, category, depth).await?;
    let mut conn =
        state.replicas.conn(wiki, &["page", "categorylinks", "linktarget", "flaggedpages", "revision"]).await?;
    let sql = "SELECT lt.lt_title, p.page_title, p.page_len, r.rev_len, fp.fp_stable
         FROM linktarget lt
         JOIN categorylinks cl ON cl.cl_target_id=lt.lt_id
         JOIN page p ON p.page_id=cl.cl_from
         JOIN flaggedpages fp ON fp.fp_page_id=p.page_id
         JOIN revision r ON r.rev_id=fp.fp_stable
         WHERE lt.lt_namespace=14 AND p.page_namespace=0 AND p.page_latest<>fp.fp_stable
           AND lt.lt_title IN";
    let (rows, truncated) = sighting::select_capped::<(String, String, u64, Option<u64>, u64), _>(
        &mut conn,
        sql,
        "ORDER BY lt.lt_title, p.page_title",
        &[],
        &tree,
        sighting::MAX_PAGES,
    )
    .await?;

    let mut by_category: HashMap<String, Vec<(String, i64, u64)>> = HashMap::new();
    for (category, title, page_len, rev_len, stable) in rows {
        let diff = page_len as i64 - rev_len.unwrap_or(page_len) as i64;
        by_category.entry(category).or_default().push((title, diff, stable));
    }

    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for category in &tree {
        let Some(pages) = by_category.get(category) else {
            continue;
        };
        for (title, diff, stable) in pages {
            let display = crate::replicas::display_title(title);
            if !seen.insert(display.clone()) {
                continue;
            }
            hits.push(Hit { category: category.clone(), title: display, diff: *diff, stable: *stable, note: None });
        }
    }

    let mut notes_capped = false;
    if extended {
        for (index, hit) in hits.iter_mut().enumerate() {
            if index >= MAX_NOTES {
                notes_capped = true;
                break;
            }
            hit.note = note(state, wiki, &hit.title, hit.stable).await;
        }
    }

    let mut groups = Vec::new();
    for category in &tree {
        let pages: Vec<PageRow> = hits
            .iter()
            .filter(|hit| &hit.category == category)
            .map(|hit| PageRow {
                title: hit.title.clone(),
                url: sighting::diff_url(wiki, &hit.title, hit.stable),
                diff: hit.diff,
                note: hit.note.clone(),
            })
            .collect();
        if pages.is_empty() {
            continue;
        }
        groups.push(Category {
            title: crate::replicas::display_title(category),
            url: sighting::pending_changes_url(wiki, category),
            pages,
        });
    }

    let sorted = size_sorted.then(|| {
        let mut pages: Vec<PageRow> = groups.iter().flat_map(|group| group.pages.iter().cloned()).collect();
        pages.sort_by(|a, b| a.diff.cmp(&b.diff).then_with(|| a.title.cmp(&b.title)));
        pages
    });
    let found = groups.iter().map(|group| group.pages.len()).sum();
    Ok((Results { groups, sorted, found, truncated }, notes_capped))
}

async fn note(state: &AppState, wiki: &Wiki, title: &str, stable: u64) -> Option<Note> {
    let revision_limit = REVISIONS.to_string();
    let json = crate::mediawiki::api(
        &state.http,
        wiki,
        &[
            ("action", "query"),
            ("prop", "revisions"),
            ("titles", title),
            ("rvlimit", &revision_limit),
            ("rvprop", "user|ids|size"),
        ],
    )
    .await
    .ok()?;
    let revisions = json["query"]["pages"][0]["revisions"].as_array()?;
    let current_size = revisions.first()?["size"].as_i64()?;
    let mut last_author = "";
    let mut authors = Vec::new();
    for (revisions_since, revision) in revisions.iter().enumerate() {
        if revision["revid"].as_u64() == Some(stable) {
            return Some(Note {
                diff: current_size - revision["size"].as_i64()?,
                revisions: revisions_since,
                authors,
                too_old: false,
            });
        }
        let author = revision["user"].as_str().unwrap_or_default();
        if !author.is_empty() && author != last_author {
            authors.insert(0, author.to_string());
        }
        last_author = author;
    }
    Some(Note { diff: 0, revisions: revisions.len(), authors, too_old: true })
}

fn rss(request: &Request, category: &str, results: &Results) -> String {
    let title = format!("{TITLE}: {}", crate::replicas::display_title(category));
    let link = format!("{}/deep_out_of_sight/", sighting::TOOL_BASE);
    let mut body = sighting::rss_header(&title, &link, &request.language);
    for group in &results.groups {
        let mut description = String::from("<ol>");
        for page in &group.pages {
            let note = match &page.note {
                Some(note) => format!(" ({})", escape::html(&note.text())),
                None => String::new(),
            };
            description.push_str(&format!(
                "<li><a href=\"{}\">{}</a>{note}</li>",
                escape::html(&page.url),
                escape::html(&page.title)
            ));
        }
        description.push_str("</ol>");
        body.push_str(&sighting::rss_item(&format!("Category:{}", group.title), &group.url, &description));
    }
    body.push_str(sighting::rss_footer());
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn note_text() {
        let note = Note { diff: 12, revisions: 2, authors: vec!["A".into(), "B".into()], too_old: false };
        assert_eq!(note.text(), "+12 chars difference; 2 revisions since last sight; edited by A, B");
        assert_eq!(Note { too_old: true, ..note }.text(), format!("more than {REVISIONS} revisions old"));
    }

    #[test]
    fn page_diff_text() {
        let page = |diff| PageRow { title: "T".into(), url: "U".into(), diff, note: None };
        assert_eq!(page(0).diff_text(), "±0");
        assert_eq!(page(3).diff_text(), "+3");
        assert_eq!(page(-3).diff_text(), "-3");
        assert_eq!(page(3).diff_class(), "diff-plus");
        assert_eq!(page(-3).diff_class(), "diff-minus");
        assert_eq!(page(0).diff_class(), "");
    }

    #[tokio::test]
    async fn form_renders() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/deep_out_of_sight/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("<h1>Deep out-of-sight</h1>"));
        assert!(page.body.contains("name=\"extended\""));
        assert!(page.body.contains("name=\"sortby\""));
    }
}
