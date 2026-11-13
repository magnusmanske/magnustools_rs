//! `catnap.php`: the articles in a category, grouped by the other categories
//! they are in.

use crate::replicas::{CATEGORY_TABLES, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;

const MAX_PAGES: usize = 1000;
const DEFAULT_MIN_GROUP: usize = 5;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language")]
    language: String,
    #[serde(default = "default_project")]
    project: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    ignore: String,
    min_group: Option<String>,
    menu: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

impl Request {
    fn min_group(&self) -> usize {
        self.min_group.as_deref().and_then(|n| n.trim().parse().ok()).unwrap_or(DEFAULT_MIN_GROUP).max(1)
    }
}

struct PageLink {
    title: String,
    url: String,
}

struct Group {
    title: String,
    url: String,
    self_url: String,
    pages: Vec<PageLink>,
}

#[derive(Template)]
#[template(path = "tools/catnap.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    min_group: usize,
    category: String,
    groups: Vec<Group>,
    other: Vec<PageLink>,
    pages: usize,
    truncated: bool,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let min_group = request.min_group();
    let language = request.language.trim().to_lowercase();
    let project = request.project.trim().to_lowercase();
    let mut page = Page {
        title: "CatNap",
        request: &request,
        min_group,
        category: request.category.trim().to_string(),
        groups: vec![],
        other: vec![],
        pages: 0,
        truncated: false,
        error: None,
    };
    if request.category.trim().is_empty() || request.menu.is_some() {
        return render(&page);
    }
    let wiki = match Wiki::new(&language, &project) {
        Ok(wiki) => wiki,
        Err(e) => {
            page.error = Some(format!("Not a wiki: {e}."));
            return render(&page);
        }
    };
    page.category = request.category.trim().to_string();
    let result = load(&state, &wiki, &request, min_group).await?;
    page.groups = result.groups;
    page.other = result.other;
    page.pages = result.pages;
    page.truncated = result.truncated;
    render(&page)
}

struct Loaded {
    groups: Vec<Group>,
    other: Vec<PageLink>,
    pages: usize,
    truncated: bool,
}

async fn load(state: &AppState, wiki: &Wiki, request: &Request, min_group: usize) -> Result<Loaded, AppError> {
    let raw_category = request.category.trim();
    let raw_category = crate::mediawiki::strip_namespace(raw_category, &["category"]).unwrap_or(raw_category);
    let category = crate::replicas::db_title(raw_category);
    let mut conn = state.replicas.conn(wiki, CATEGORY_TABLES).await?;
    let pages: Vec<(u64, String)> = conn
        .exec(
            "SELECT p.page_id, p.page_title FROM page p
             JOIN categorylinks cl ON cl.cl_from=p.page_id
             JOIN linktarget lt ON lt.lt_id=cl.cl_target_id
             WHERE lt.lt_namespace=14 AND lt.lt_title=? AND p.page_namespace=0
             ORDER BY p.page_title LIMIT ?",
            (category.as_str(), MAX_PAGES),
        )
        .await
        .map_err(anyhow::Error::from)?;
    if pages.is_empty() {
        return Ok(Loaded { groups: vec![], other: vec![], pages: 0, truncated: false });
    }
    let ids: Vec<u64> = pages.iter().map(|(id, _)| *id).collect();
    let links: Vec<(u64, String)> = select_in(
        &mut conn,
        "SELECT cl.cl_from, lt.lt_title FROM categorylinks cl
         JOIN linktarget lt ON lt.lt_id=cl.cl_target_id
         WHERE lt.lt_namespace=14 AND cl.cl_from IN",
        "",
        &[],
        &ids,
    )
    .await?;

    let ignore: Vec<String> = request
        .ignore
        .lines()
        .map(|line| line.trim_end_matches('\r').trim().replace(' ', "_"))
        .filter(|line| !line.is_empty())
        .collect();
    let groups = group_pages(&pages, &links, &ignore, &category, min_group);

    let page_index: HashMap<u64, &String> = pages.iter().map(|(id, title)| (*id, title)).collect();
    let listed: HashSet<u64> = groups.values().flatten().copied().collect();
    let mut other: Vec<PageLink> =
        pages.iter().filter(|(id, _)| !listed.contains(id)).map(|(_, title)| page_link(wiki, title)).collect();
    other.sort_by(|a, b| a.title.cmp(&b.title));
    let _ = page_index;

    let groups = groups
        .into_iter()
        .map(|(name, mut ids)| {
            ids.sort_by_key(|id| page_index.get(id).map(|title| title.as_str()).unwrap_or_default());
            let title = crate::replicas::display_title(&name);
            Group {
                url: wiki.page_url(&format!("Category:{title}")),
                self_url: format!(
                    "/catnap/?category={}&language={}&project={}&min_group={}",
                    encode_component(&title),
                    encode_component(&request.language.trim().to_lowercase()),
                    encode_component(&request.project.trim().to_lowercase()),
                    min_group
                ),
                title,
                pages: ids
                    .into_iter()
                    .filter_map(|id| page_index.get(&id).map(|title| page_link(wiki, title)))
                    .collect(),
            }
        })
        .collect();

    Ok(Loaded { groups, other, pages: pages.len(), truncated: pages.len() >= MAX_PAGES })
}

fn page_link(wiki: &Wiki, title: &str) -> PageLink {
    let title = crate::replicas::display_title(title);
    PageLink { url: wiki.page_url(&title), title }
}

/// Groups the pages by their other categories. Categories in `ignore`, the
/// category itself, and groups smaller than `min_group` are left out.
fn group_pages(
    pages: &[(u64, String)],
    links: &[(u64, String)],
    ignore: &[String],
    category: &str,
    min_group: usize,
) -> BTreeMap<String, Vec<u64>> {
    let known: HashSet<u64> = pages.iter().map(|(id, _)| *id).collect();
    let mut groups: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for (page_id, name) in links {
        if known.contains(page_id) && name != category && !ignore.contains(name) {
            groups.entry(name.clone()).or_default().push(*page_id);
        }
    }
    groups.retain(|name, ids| name != category && ids.len() >= min_group);
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages() -> Vec<(u64, String)> {
        vec![(1, "A".into()), (2, "B".into()), (3, "C".into())]
    }

    #[test]
    fn groups_and_ignores_categories() {
        let links = vec![
            (1, "X".into()),
            (2, "X".into()),
            (1, "Y".into()),
            (2, "Z".into()),
            (3, "Z".into()),
            (3, "Ignored".into()),
        ];
        let groups = group_pages(&pages(), &links, &["Ignored".into()], "X", 2);
        assert_eq!(groups.get("X"), None, "the category itself is left out");
        assert_eq!(groups.get("Y"), None, "one page is below the minimum");
        assert_eq!(groups.get("Z").map(Vec::len), Some(2));
        assert!(!groups.contains_key("Ignored"));
    }

    #[test]
    fn minimum_group_is_at_least_one() {
        let request = Request {
            language: "en".into(),
            project: "wikipedia".into(),
            category: "X".into(),
            ignore: String::new(),
            min_group: Some("0".into()),
            menu: None,
        };
        assert_eq!(request.min_group(), 1);
    }

    #[test]
    fn category_titles_are_database_titles() {
        assert_eq!(crate::replicas::db_title("leuchtturm in schottland"), "Leuchtturm_in_schottland");
    }
}
