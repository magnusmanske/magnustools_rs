//! `deep_insight.php`: pages in a category tree that have never been sighted.
//! The original's redirect mode relied on database columns that no longer
//! exist; here it shows the redirects among the category's pages.

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

const TITLE: &str = "Deep InSight";
const DEFAULT_DEPTH: u32 = 0;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    project: String,
    category: String,
    depth: Option<String>,
    mode: Option<String>,
    redirects: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            language: sighting::default_language(),
            project: sighting::default_project(),
            category: String::new(),
            depth: None,
            mode: None,
            redirects: None,
        }
    }
}

struct PageLink {
    title: String,
    url: String,
}

struct Category {
    title: String,
    url: String,
    pages: Vec<PageLink>,
}

struct Results {
    categories: Vec<Category>,
    found: usize,
    truncated: bool,
}

#[derive(Template)]
#[template(path = "tools/deep_insight.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    depth: u32,
    rss: bool,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let wiki = Wiki::new(&request.language, &request.project)?;
    let depth = sighting::depth(request.depth.as_deref(), DEFAULT_DEPTH);
    let rss_output = sighting::is_rss(request.mode.as_deref());
    let category = request.category.trim();
    if category.is_empty() {
        return Ok(
            render(&Page { title: TITLE, request: &request, depth, rss: rss_output, results: None })?.into_response()
        );
    }
    let results = results(&state, &wiki, category, depth, request.redirects.is_some()).await?;
    if rss_output {
        return Ok(sighting::rss_response(rss(&request, category, &results)));
    }
    Ok(render(&Page { title: TITLE, request: &request, depth, rss: rss_output, results: Some(results) })?
        .into_response())
}

async fn results(
    state: &AppState,
    wiki: &Wiki,
    category: &str,
    depth: u32,
    redirects: bool,
) -> Result<Results, AppError> {
    let tree = state.replicas.category_tree(wiki, category, depth).await?;
    let mut conn = state.replicas.conn(wiki, &["page", "categorylinks", "linktarget", "flaggedpages"]).await?;
    let redirects = if redirects { "AND page_is_redirect=1" } else { "" };
    let sql = format!(
        "SELECT lt_title, page_title FROM linktarget
         JOIN categorylinks ON cl_target_id=lt_id
         JOIN page ON cl_from=page_id
         LEFT JOIN flaggedpages ON fp_page_id=page_id
         WHERE lt_namespace=14 AND page_namespace=0 AND fp_page_id IS NULL {redirects}
           AND lt_title IN"
    );
    let (rows, truncated) = sighting::select_capped::<(String, String), _>(
        &mut conn,
        &sql,
        "ORDER BY lt_title, page_title",
        &[],
        &tree,
        sighting::MAX_PAGES,
    )
    .await?;

    let mut by_category: HashMap<String, Vec<String>> = HashMap::new();
    for (category, title) in rows {
        by_category.entry(category).or_default().push(title);
    }
    let mut seen = HashSet::new();
    let mut found = 0;
    let mut categories = Vec::new();
    for category in &tree {
        let Some(pages) = by_category.get(category) else {
            continue;
        };
        let mut links = Vec::new();
        for title in pages {
            let display = crate::replicas::display_title(title);
            if !seen.insert(display.clone()) {
                continue;
            }
            found += 1;
            links.push(PageLink { title: display, url: sighting::page_url(wiki, title, true) });
        }
        if links.is_empty() {
            continue;
        }
        categories.push(Category {
            title: crate::replicas::display_title(category),
            url: sighting::pending_changes_url(wiki, category),
            pages: links,
        });
    }
    Ok(Results { categories, found, truncated })
}

fn rss(request: &Request, category: &str, results: &Results) -> String {
    let title = format!("{TITLE}: {}", crate::replicas::display_title(category));
    let link = format!("{}/deep_insight/", sighting::TOOL_BASE);
    let mut body = sighting::rss_header(&title, &link, &request.language);
    for category in &results.categories {
        let mut description = String::from("<ol>");
        for page in &category.pages {
            description.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>",
                escape::html(&page.url),
                escape::html(&page.title)
            ));
        }
        description.push_str("</ol>");
        body.push_str(&sighting::rss_item(&format!("Category:{}", category.title), &category.url, &description));
    }
    body.push_str(sighting::rss_footer());
    body
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::get;

    #[tokio::test]
    async fn form_renders() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/deep_insight/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("<h1>Deep InSight</h1>"));
        assert!(page.body.contains("name=\"redirects\""));
    }

    #[tokio::test]
    async fn old_address_redirects() {
        let dir = tempfile::tempdir().unwrap();
        let old = get(dir.path(), "/deep_insight.php?category=Mann&depth=0").await;
        assert_eq!(old.status.as_u16(), 308);
        assert_eq!(old.headers["location"], "/deep_insight/?category=Mann&depth=0");
    }
}
